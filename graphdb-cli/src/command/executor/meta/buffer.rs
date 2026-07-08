use crate::command::executor::CommandExecutor;
use crate::input::buffer::{self};
use crate::session::manager::SessionManager;
use crate::utils::error::Result;

pub fn execute_edit(
    executor: &mut CommandExecutor,
    file: Option<&str>,
    line: Option<usize>,
    session_mgr: &SessionManager,
) -> Result<bool> {
    let editor = session_mgr
        .session()
        .and_then(|s| s.get_variable("EDITOR").cloned());
    let editor_ref = editor.as_deref();
    let result =
        buffer::edit_in_external_editor(executor.query_buffer_mut(), file, line, editor_ref)?;
    if result {
        let content = executor.query_buffer().content();
        if !content.trim().is_empty() {
            executor.write_output(&content)?;
        }
    }
    Ok(true)
}

pub fn execute_print_buffer(executor: &mut CommandExecutor) -> Result<bool> {
    let content = executor.query_buffer().content();
    if content.trim().is_empty() {
        executor.write_output("(query buffer is empty)")?;
    } else {
        executor.write_output(&content)?;
    }
    Ok(true)
}

pub fn execute_reset_buffer(executor: &mut CommandExecutor) -> Result<bool> {
    executor.query_buffer_mut().reset();
    executor.write_output("Query buffer reset.")?;
    Ok(true)
}

pub fn execute_write_buffer(executor: &mut CommandExecutor, file: &str) -> Result<bool> {
    buffer::write_buffer_to_file(executor.query_buffer(), file)?;
    executor.write_output(&format!("Query buffer written to: {}", file))?;
    Ok(true)
}
