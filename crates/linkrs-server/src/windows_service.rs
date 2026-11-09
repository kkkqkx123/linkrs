//! Windows service integration.
//!
//! Peer to `systemd` on Linux: readiness is reported after listeners are
//! bound, and the stop control funnels into the shared graceful shutdown
//! path. Interactive runs are unaffected; the dispatcher falls back to the
//! console path when the process was not started by the Service Control
//! Manager.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use log::{debug, error};
use parking_lot::Mutex;
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle},
    service_dispatcher, Error as ServiceError,
};

use crate::config::Config;

/// Service name registered with the Service Control Manager.
pub const SERVICE_NAME: &str = "linkrs";

/// Win32 error reported when a process that was not started by the Service
/// Control Manager calls into the dispatcher; it selects the console fallback.
const FAILED_SERVICE_CONTROLLER_CONNECT: i32 = 1063;

/// Stop-pending hint given to the manager. The graceful shutdown path
/// enforces its own shorter timeout before this elapses.
const STOP_WAIT_HINT: Duration = Duration::from_secs(60);

static PENDING_STARTUP: OnceLock<Mutex<Option<(Config, Option<PathBuf>)>>> = OnceLock::new();
static STATUS_HANDLE: OnceLock<Mutex<Option<ServiceStatusHandle>>> = OnceLock::new();
static EXIT_CODE: AtomicI32 = AtomicI32::new(0);

define_windows_service!(ffi_service_main, service_main);

/// Outcome of handing startup to the Service Control Manager.
pub enum ServeDispatch {
    /// Ran to completion as a service; the process exits with this code.
    Serviced(i32),
    /// Not started by the manager; the caller continues on the console path.
    Console {
        config: Config,
        config_path: Option<PathBuf>,
    },
}

/// Hand startup to the Service Control Manager, falling back to console mode
/// when the process runs interactively.
pub fn dispatch_serve(config: Config, config_path: Option<PathBuf>) -> ServeDispatch {
    *pending_slot().lock() = Some((config, config_path));
    match service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
        Ok(()) => ServeDispatch::Serviced(EXIT_CODE.load(Ordering::SeqCst)),
        Err(err) if is_console_fallback(&err) => {
            debug!("not started by the Service Control Manager; using console mode");
            match pending_slot().lock().take() {
                Some((config, config_path)) => ServeDispatch::Console {
                    config,
                    config_path,
                },
                None => {
                    error!("console fallback lost its pending startup");
                    ServeDispatch::Serviced(1)
                }
            }
        }
        Err(err) => {
            error!("failed to connect to the Service Control Manager: {}", err);
            ServeDispatch::Serviced(1)
        }
    }
}

/// Notify the manager that listeners are bound and the service is running.
/// No-op when running on the console path.
pub fn notify_ready() {
    report_status(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        ServiceExitCode::NO_ERROR,
        Duration::default(),
    );
}

/// Notify the manager that graceful shutdown has started.
/// No-op when running on the console path.
pub fn notify_stopping() {
    report_status(
        ServiceState::StopPending,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        ServiceExitCode::NO_ERROR,
        STOP_WAIT_HINT,
    );
}

fn pending_slot() -> &'static Mutex<Option<(Config, Option<PathBuf>)>> {
    PENDING_STARTUP.get_or_init(|| Mutex::new(None))
}

fn handle_slot() -> &'static Mutex<Option<ServiceStatusHandle>> {
    STATUS_HANDLE.get_or_init(|| Mutex::new(None))
}

fn report_status(
    state: ServiceState,
    accepted: ServiceControlAccept,
    exit_code: ServiceExitCode,
    wait_hint: Duration,
) {
    let handle = *handle_slot().lock();
    let Some(handle) = handle else {
        debug!("skipping service status report: no control handle (console mode?)");
        return;
    };
    let status = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accepted,
        exit_code,
        checkpoint: 0,
        wait_hint,
        process_id: None,
    };
    if let Err(err) = handle.set_service_status(status) {
        debug!("failed to report service status: {}", err);
    }
}

fn report_and_store_exit(state: ServiceState, code: u32) {
    let exit_code = if code == 0 {
        ServiceExitCode::NO_ERROR
    } else {
        ServiceExitCode::Win32(code)
    };
    EXIT_CODE.store(code as i32, Ordering::SeqCst);
    report_status(
        state,
        ServiceControlAccept::empty(),
        exit_code,
        Duration::default(),
    );
}

fn is_console_fallback(err: &ServiceError) -> bool {
    matches!(
        err,
        ServiceError::Winapi(io) if io.raw_os_error() == Some(FAILED_SERVICE_CONTROLLER_CONNECT)
    )
}

fn service_main(_arguments: Vec<OsString>) {
    let event_handler = |control_event| match control_event {
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        ServiceControl::Stop | ServiceControl::Shutdown => {
            report_status(
                ServiceState::StopPending,
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
                ServiceExitCode::NO_ERROR,
                STOP_WAIT_HINT,
            );
            crate::shutdown::request_shutdown();
            ServiceControlHandlerResult::NoError
        }
        _ => ServiceControlHandlerResult::NotImplemented,
    };

    let handle = match service_control_handler::register(SERVICE_NAME, event_handler) {
        Ok(handle) => handle,
        Err(err) => {
            error!("failed to register service control handler: {}", err);
            EXIT_CODE.store(1, Ordering::SeqCst);
            return;
        }
    };
    *handle_slot().lock() = Some(handle);
    report_status(
        ServiceState::StartPending,
        ServiceControlAccept::empty(),
        ServiceExitCode::NO_ERROR,
        STOP_WAIT_HINT,
    );

    let startup = pending_slot().lock().take();
    let Some((config, config_path)) = startup else {
        error!("service started without pending startup");
        report_and_store_exit(ServiceState::Stopped, 1);
        return;
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            error!("failed to create tokio runtime: {}", err);
            report_and_store_exit(ServiceState::Stopped, 1);
            return;
        }
    };
    let exit_code = match runtime.block_on(crate::startup::start_service_with_config_path(
        config,
        config_path,
    )) {
        Ok(()) => 0,
        Err(err) => {
            error!("service failed: {}", err);
            1
        }
    };
    report_and_store_exit(ServiceState::Stopped, exit_code);
}
