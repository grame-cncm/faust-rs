//! Shared support for Faust foreign-function interfaces.
//!
//! This dependency-light crate centralizes representation and marshalling
//! mechanics reused by the Box, Signal, Interpreter, Cranelift, tree-handle,
//! and WebAssembly adapters. It does not export C symbols and it does not own
//! backend-specific factory or instance semantics.
//!
//! Compiler-core crates must not depend on this crate. Keeping the dependency
//! direction one-way confines raw-pointer operations and ABI policy to the
//! foreign boundary.
//!
//! # API mapping status
//! The crate name and Rust module paths are `adapted` internal APIs. The
//! definitions and helper behavior preserve the pre-split `utils` crate, so the
//! external C ABI is unchanged.

pub mod abi;
pub mod args;
pub mod factory_cache;
pub mod memory;
pub mod sha1;
pub mod strings;

pub use abi::{FfiFaustFloat, MetaGlue, UIGlue};
pub use args::{FfiCompileArgs, parse_ffi_compile_args};
pub use factory_cache::{FactoryCache, FactoryHandle, FactoryRelease};
pub use memory::{alloc_opaque, free_opaque};
pub use sha1::{sha1, sha1_hex};
pub use strings::{
    alloc_c_string, decode_c_argv, free_c_memory_c_string_only, free_c_string, null_c_string_array,
    optional_c_string_arg, required_c_string_arg, write_error_4096,
};
