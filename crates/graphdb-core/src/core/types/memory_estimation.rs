pub trait MemoryEstimatable {
    fn estimate_memory(&self) -> usize;
}

pub fn estimate_string_memory(s: &String) -> usize {
    std::mem::size_of::<String>() + s.capacity()
}

pub fn estimate_option_string_memory(opt: &Option<String>) -> usize {
    std::mem::size_of::<Option<String>>()
        + opt
            .as_ref()
            .map(|s| std::mem::size_of::<String>() + s.capacity())
            .unwrap_or(0)
}

pub fn estimate_vec_string_memory(vec: &[String]) -> usize {
    std::mem::size_of::<Vec<String>>()
        + vec
            .iter()
            .map(|s| std::mem::size_of::<String>() + s.capacity())
            .sum::<usize>()
}

pub fn estimate_vec_memory<T>(_vec: &[T]) -> usize {
    std::mem::size_of_val(_vec)
}
