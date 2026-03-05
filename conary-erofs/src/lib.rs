// conary-erofs/src/lib.rs
//! EROFS filesystem image builder
//!
//! Produces valid EROFS images for use with Linux composefs.
//! Supports compression (LZ4, LZMA), inline data, tail packing,
//! and chunk-based external file references.

pub mod builder;
pub mod chunk;
pub mod compress;
pub mod dirent;
pub mod inode;
pub mod superblock;
pub mod tail_pack;
pub mod verify;
pub mod xattr;
