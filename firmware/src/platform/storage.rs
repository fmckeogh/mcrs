#[path = "storage/littlefs.rs"]
mod littlefs;
pub use littlefs::LittleFsStorage;

pub use mcrs_repeater::platform::storage::{Error, Storage};

#[derive(Clone, Copy)]
pub struct Layout {
    pub partition_label: &'static str,
    pub partition_size: usize,
    pub max_file_size: usize,
}
