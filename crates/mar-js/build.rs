//! Compile the prelude to QuickJS bytecode at build time.
//!
//! The prelude is two hundred kilobytes of JavaScript, and parsing it was
//! most of what an empty page cost: eleven milliseconds of a twenty
//! millisecond render. Loading the same code as bytecode takes under one.
//! The source is stripped from the bytecode, which also makes every prelude
//! function print as `[native code]`, the way a browser's do.
//!
//! Bytecode is tied to the QuickJS this crate links, and that is the same
//! crate that compiles it here, so the two cannot drift apart. It is written
//! little-endian; every target this project builds for is.

use rquickjs::{Context, Module, Runtime, WriteOptions, WriteOptionsEndianness};
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=src/prelude.js");
    let source = std::fs::read_to_string("src/prelude.js").expect("prelude.js is readable");
    let runtime = Runtime::new().expect("a QuickJS runtime");
    let context = Context::full(&runtime).expect("a QuickJS context");
    let bytes = context.with(|ctx| {
        let module = Module::declare(ctx.clone(), "mar:prelude", source.as_str())
            .unwrap_or_else(|e| panic!("the prelude does not compile: {e}"));
        module
            .write(WriteOptions {
                endianness: WriteOptionsEndianness::Little,
                strip_source: true,
                ..WriteOptions::default()
            })
            .expect("the prelude serialises")
    });
    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("prelude.qjsbc");
    std::fs::write(&out, bytes).expect("the bytecode is written");
}
