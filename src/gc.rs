//! Garbage collector statistics

#[derive(Debug, Default, Clone, Copy)]
pub struct GCStats {
    pub collections: u64,
    pub objects_freed: u64,
    pub strings_freed: u64,
}
