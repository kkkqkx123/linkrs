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

pub(crate) use forward_methods;
