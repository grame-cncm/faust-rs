use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};

use clap::Parser;
use cranelift_ffi::factory::{
    createCCraneliftDSPFactoryFromFile, deleteCCraneliftDSPFactory,
    writeCCraneliftDSPFactoryToBitcodeFile,
};

#[derive(Debug, Parser)]
#[command(
    name = "export_clif",
    about = "Compile a Faust DSP and export Cranelift bitcode"
)]
struct CliArgs {
    /// Input Faust DSP source.
    #[arg(value_name = "INPUT_DSP")]
    input: PathBuf,

    /// Output Cranelift bitcode file.
    #[arg(value_name = "OUTPUT_CLIF")]
    output: PathBuf,
}

fn main() -> Result<(), String> {
    let args = CliArgs::parse();
    let input_c = path_to_cstring(&args.input)?;
    let output_c = path_to_cstring(&args.output)?;
    let mut err = [0_i8; 4096];

    let factory = unsafe {
        createCCraneliftDSPFactoryFromFile(
            input_c.as_ptr(),
            0,
            std::ptr::null(),
            err.as_mut_ptr(),
            1,
        )
    };
    if factory.is_null() {
        let msg = unsafe { CStr::from_ptr(err.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        return Err(format!("createCCraneliftDSPFactoryFromFile failed: {msg}"));
    }

    let ok = unsafe { writeCCraneliftDSPFactoryToBitcodeFile(factory, output_c.as_ptr()) };
    if !ok {
        unsafe {
            let _ = deleteCCraneliftDSPFactory(factory);
        }
        return Err("writeCCraneliftDSPFactoryToBitcodeFile failed".to_owned());
    }

    unsafe {
        let _ = deleteCCraneliftDSPFactory(factory);
    }

    println!("{}", args.output.display());
    Ok(())
}

#[cfg(unix)]
fn path_to_cstring(path: &Path) -> Result<CString, String> {
    use std::os::unix::ffi::OsStrExt;

    CString::new(path.as_os_str().as_bytes())
        .map_err(|error| format!("path contains an interior NUL: {error}"))
}

#[cfg(not(unix))]
fn path_to_cstring(path: &Path) -> Result<CString, String> {
    CString::new(path.to_string_lossy().as_bytes())
        .map_err(|error| format!("path contains an interior NUL: {error}"))
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::CliArgs;

    #[test]
    fn cli_requires_exactly_two_paths() {
        CliArgs::command().debug_assert();
        assert!(CliArgs::try_parse_from(["export_clif", "in.dsp", "out.clif"]).is_ok());
        assert!(CliArgs::try_parse_from(["export_clif", "in.dsp"]).is_err());
        assert!(CliArgs::try_parse_from(["export_clif", "in.dsp", "out.clif", "extra"]).is_err());
    }
}
