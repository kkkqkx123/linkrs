use super::super::CustomFunction;
use super::FunctionRegistry;

impl FunctionRegistry {
    /// Registering a custom function (full form)
    pub fn register_custom_full(&mut self, function: CustomFunction) {
        let upper_name = function.name.to_uppercase();
        self.custom_functions.insert(upper_name, function);
    }

    /// Remove a non-dynamic custom function by name.
    ///
    /// Dynamic UDFs must go through `unload_dynamic_udf` so the library
    /// handle is dropped exactly once; builtins are never removed here.
    /// Returns true when an entry was removed.
    pub fn unregister_custom(&mut self, name: &str) -> bool {
        let upper = name.to_uppercase();
        if self.dynamic_libraries.contains_key(&upper)
            || self.builtin_functions.contains_key(&upper)
        {
            return false;
        }
        self.custom_functions.remove(&upper).is_some()
    }
    /// Obtaining a custom function
    pub fn get_custom(&self, name: &str) -> Option<&CustomFunction> {
        // Convert to uppercase for case-insensitive lookup
        let upper_name = name.to_uppercase();
        self.custom_functions.get(&upper_name)
    }
}
