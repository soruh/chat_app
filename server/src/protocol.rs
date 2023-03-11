#[repr(C)]
pub struct Prelude {
    version: [u8; 8],
    commit_hash: [u8; 20],
}

#[repr(C)]
pub struct Header {
    kind: u8,
}
