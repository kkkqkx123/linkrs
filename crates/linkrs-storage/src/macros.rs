//! Shared macros for storage decorator implementations.

macro_rules! forward_methods {
    ($field:ident; $(fn $fn:ident(&self $(, $arg:ident : $ty:ty)* $(,)?);)+) => {
        $(
            fn $fn(&self, $($arg: $ty),*) {
                self.$field.$fn($($arg),*)
            }
        )+
    };
    ($field:ident; $(fn $fn:ident(&mut self $(, $arg:ident : $ty:ty)* $(,)?);)+) => {
        $(
            fn $fn(&mut self, $($arg: $ty),*) {
                self.$field.$fn($($arg),*)
            }
        )+
    };
    ($field:ident; $(fn $fn:ident(&self $(, $arg:ident : $ty:ty)* $(,)?) -> $ret:ty;)+) => {
        $(
            fn $fn(&self, $($arg: $ty),*) -> $ret {
                self.$field.$fn($($arg),*)
            }
        )+
    };
    ($field:ident; $(fn $fn:ident(&mut self $(, $arg:ident : $ty:ty)* $(,)?) -> $ret:ty;)+) => {
        $(
            fn $fn(&mut self, $($arg: $ty),*) -> $ret {
                self.$field.$fn($($arg),*)
            }
        )+
    };
}

/// Forward [`StorageWriter`] methods to a field, timing each call and
/// recording storage errors — the MetricsStorage decorator boilerplate.
macro_rules! forward_timed_write_methods {
    ($field:ident; $(fn $fn:ident(&mut self $(, $arg:ident : $ty:ty)* $(,)?) -> $ret:ty;)+) => {
        $(
            fn $fn(&mut self, $($arg: $ty),*) -> $ret {
                let start = std::time::Instant::now();
                let result = StorageWriter::$fn(&mut self.$field, $($arg),*);
                if result.is_err() {
                    if let Some(stats) = &self.stats {
                        stats.record_storage_error();
                    }
                }
                self.record_write(start);
                result
            }
        )+
    };
}

/// Forward [`StorageReader`] methods to a field, timing each call and
/// recording storage errors — the MetricsStorage decorator boilerplate.
macro_rules! forward_timed_read_methods {
    ($field:ident; $(fn $fn:ident(&self $(, $arg:ident : $ty:ty)* $(,)?) -> $ret:ty;)+) => {
        $(
            fn $fn(&self, $($arg: $ty),*) -> $ret {
                let start = std::time::Instant::now();
                let result = StorageReader::$fn(&self.$field, $($arg),*);
                if result.is_err() {
                    if let Some(stats) = &self.stats {
                        stats.record_storage_error();
                    }
                }
                self.record_read(start);
                result
            }
        )+
    };
}

/// Forward [`linkrs_transaction::UndoTarget`] methods to a field — the
/// MetricsStorage decorator boilerplate.
macro_rules! forward_undo_methods {
    ($field:ident; $(fn $fn:ident(&self $(, $arg:ident : $ty:ty)* $(,)?) -> $ret:ty;)+) => {
        $(
            fn $fn(&self, $($arg: $ty),*) -> $ret {
                linkrs_transaction::UndoTarget::$fn(&self.$field, $($arg),*)
            }
        )+
    };
}

pub(crate) use forward_methods;
pub(crate) use forward_timed_read_methods;
pub(crate) use forward_timed_write_methods;
pub(crate) use forward_undo_methods;
