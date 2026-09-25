//! Tests of the factory C API.

use std::ffi::{CStr, CString};

use super::{
    canonicalize_cache_identity_argv, clearCCraneliftForeignFunctions,
    createCCraneliftDSPFactoryFromBoxes, createCCraneliftDSPFactoryFromFile,
    createCCraneliftDSPFactoryFromSignals, createCCraneliftDSPFactoryFromString,
    deleteAllCCraneliftDSPFactories, deleteCCraneliftDSPFactory, factory_status, freeCMemory,
    getAllCCraneliftDSPFactories, getCCraneliftDSPFactoryCompileOptions,
    getCCraneliftDSPFactoryFromSHAKey, getCCraneliftDSPFactoryJSON, getCCraneliftDSPFactoryName,
    getCCraneliftDSPFactorySHAKey, getCLibFaustVersion, readCCraneliftDSPFactoryFromBitcode,
    readCCraneliftDSPFactoryFromBitcodeFile, registerCCraneliftForeignFunction,
    unregisterCCraneliftForeignFunction, writeCCraneliftDSPFactoryToBitcode,
    writeCCraneliftDSPFactoryToBitcodeFile,
};
use crate::instance::createCCraneliftDSPInstance;

extern "C" fn ffi_test_foreign_gain(x: f32) -> f32 {
    x * 0.25
}

#[test]
fn factory_scaffold_status_is_stable() {
    let _guard = crate::test_serial_guard();
    assert_eq!(factory_status(), "cranelift-ffi factory runtime");
}

#[test]
fn version_symbol_returns_static_c_string() {
    let _guard = crate::test_serial_guard();
    let ptr = getCLibFaustVersion();
    assert!(!ptr.is_null());
    let s = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap();
    assert!(s.contains("cranelift-ffi"));
}

#[test]
fn create_factory_from_string_runtime_roundtrip_queries() {
    let _guard = crate::test_serial_guard();
    super::clear_registered_foreign_functions();
    let name = c"mydsp";
    let src = c"process = _;";
    let args = [c"-vec"];
    let argv = [args[0].as_ptr()];
    let mut err = [0_i8; 4096];

    let factory = unsafe {
        createCCraneliftDSPFactoryFromString(
            name.as_ptr(),
            src.as_ptr(),
            1,
            argv.as_ptr(),
            err.as_mut_ptr(),
            2,
        )
    };
    assert!(!factory.is_null());

    let name_ptr = unsafe { getCCraneliftDSPFactoryName(factory) };
    let json_ptr = unsafe { getCCraneliftDSPFactoryJSON(factory) };
    let opts_ptr = unsafe { getCCraneliftDSPFactoryCompileOptions(factory) };
    assert!(!name_ptr.is_null());
    assert!(!json_ptr.is_null());
    assert!(!opts_ptr.is_null());

    let name_s = unsafe { CStr::from_ptr(name_ptr) }.to_str().unwrap();
    let json_s = unsafe { CStr::from_ptr(json_ptr) }.to_str().unwrap();
    let opts_s = unsafe { CStr::from_ptr(opts_ptr) }.to_str().unwrap();
    assert_eq!(name_s, "mydsp");
    assert!(json_s.contains("\"backend\": \"cranelift\""));
    assert!(opts_s.contains("opt_level=2"));

    unsafe {
        assert!((*factory).compiled_jit.is_some());
        let lowered = (*factory).compute_body_lowered;
        assert!(json_s.contains(&format!(
            "\"compute_body_lowered\": {}",
            if lowered { "true" } else { "false" }
        )));
        freeCMemory(name_ptr.cast());
        freeCMemory(json_ptr.cast());
        freeCMemory(opts_ptr.cast());
        assert!(deleteCCraneliftDSPFactory(factory));
    }
}

#[test]
fn cache_identity_canonicalizes_ss_value_only() {
    // `-ss 0` vs `-ss 1` decode to different strategies: distinct tokens.
    let depth_first = canonicalize_cache_identity_argv(&["-ss".to_owned(), "0".to_owned()]);
    let breadth_first = canonicalize_cache_identity_argv(&["-ss".to_owned(), "1".to_owned()]);
    assert_ne!(depth_first, breadth_first);

    // `-ss 3` and `-ss 42` both decode to `ReverseBreadthFirst`: identical
    // canonical token, even though the raw argv strings differ.
    let three = canonicalize_cache_identity_argv(&["-ss".to_owned(), "3".to_owned()]);
    let forty_two = canonicalize_cache_identity_argv(&["-ss".to_owned(), "42".to_owned()]);
    assert_eq!(three, forty_two);
    assert_eq!(three, vec!["-ss".to_owned(), "3".to_owned()]);

    // Every other token passes through unchanged.
    let mixed = canonicalize_cache_identity_argv(&[
        "-vec".to_owned(),
        "-ss".to_owned(),
        "42".to_owned(),
        "-vs".to_owned(),
        "64".to_owned(),
    ]);
    assert_eq!(
        mixed,
        vec![
            "-vec".to_owned(),
            "-ss".to_owned(),
            "3".to_owned(),
            "-vs".to_owned(),
            "64".to_owned(),
        ]
    );
}

#[test]
fn cache_identity_canonicalizes_all_mem0_aliases() {
    for spelling in ["-mem", "-mem0", "--memory-manager", "--memory-manager0"] {
        assert_eq!(
            canonicalize_cache_identity_argv(&[spelling.to_owned()]),
            vec!["-mem0".to_owned()],
            "{spelling}"
        );
    }
    assert_ne!(
        canonicalize_cache_identity_argv(&[]),
        canonicalize_cache_identity_argv(&["-mem0".to_owned()])
    );
    assert_eq!(
        canonicalize_cache_identity_argv(&["-mem".to_owned(), "--memory-manager0".to_owned(),]),
        vec!["-mem0".to_owned()]
    );
}

#[test]
fn ss_scheduling_strategy_changes_factory_cache_identity_canonically() {
    let _guard = crate::test_serial_guard();
    super::clear_registered_foreign_functions();
    let name = c"ss_cache_identity_test";
    let src = c"process = _;";

    let build = |ss_value: &std::ffi::CStr| unsafe {
        let mut err = [0_i8; 4096];
        let flag = c"-ss";
        let argv = [flag.as_ptr(), ss_value.as_ptr()];
        let factory = createCCraneliftDSPFactoryFromString(
            name.as_ptr(),
            src.as_ptr(),
            2,
            argv.as_ptr(),
            err.as_mut_ptr(),
            0,
        );
        assert!(
            !factory.is_null(),
            "factory build failed: {}",
            CStr::from_ptr(err.as_ptr()).to_string_lossy()
        );
        factory
    };

    let f_ss0 = build(c"0");
    let f_ss1 = build(c"1");
    let f_ss3 = build(c"3");
    let f_ss42 = build(c"42");

    unsafe {
        // `-ss 0` (DepthFirst) vs `-ss 1` (BreadthFirst): distinct cache identity.
        assert_ne!((*f_ss0).sha_key, (*f_ss1).sha_key);
        assert_ne!((*f_ss0).compile_options, (*f_ss1).compile_options);

        // `-ss 3` and `-ss 42` both decode to ReverseBreadthFirst: identical
        // cache identity, proving the canonical enum value — not the raw
        // argv token — drives the identity.
        assert_eq!((*f_ss3).sha_key, (*f_ss42).sha_key);
        assert_eq!((*f_ss3).compile_options, (*f_ss42).compile_options);
        assert_eq!(f_ss3, f_ss42);

        assert!(deleteCCraneliftDSPFactory(f_ss0));
        assert!(deleteCCraneliftDSPFactory(f_ss1));
        assert!(!deleteCCraneliftDSPFactory(f_ss3));
        assert!(deleteCCraneliftDSPFactory(f_ss42));
    }
}

#[test]
fn create_factory_from_file_rejects_null_filename() {
    let _guard = crate::test_serial_guard();
    let mut err = [0_i8; 4096];
    let factory = unsafe {
        createCCraneliftDSPFactoryFromFile(
            std::ptr::null(),
            0,
            std::ptr::null(),
            err.as_mut_ptr(),
            0,
        )
    };
    assert!(factory.is_null());
    let msg = unsafe { CStr::from_ptr(err.as_ptr()) }.to_str().unwrap();
    assert!(msg.contains("null filename"));
}

#[test]
fn create_factory_from_string_reports_compiler_error_for_invalid_faust() {
    let _guard = crate::test_serial_guard();
    let name = c"bad";
    let src = c"process = ;";
    let mut err = [0_i8; 4096];

    let factory = unsafe {
        createCCraneliftDSPFactoryFromString(
            name.as_ptr(),
            src.as_ptr(),
            0,
            std::ptr::null(),
            err.as_mut_ptr(),
            0,
        )
    };
    assert!(factory.is_null());
    let msg = unsafe { CStr::from_ptr(err.as_ptr()) }.to_str().unwrap();
    assert!(!msg.is_empty());
}

#[test]
fn cache_lookup_and_list_are_wired_to_created_factories() {
    let _guard = crate::test_serial_guard();
    super::clear_registered_foreign_functions();
    let name = c"cachetest";
    let src = c"process = _;";
    let mut err = [0_i8; 4096];

    let factory = unsafe {
        createCCraneliftDSPFactoryFromString(
            name.as_ptr(),
            src.as_ptr(),
            0,
            std::ptr::null(),
            err.as_mut_ptr(),
            3,
        )
    };
    assert!(!factory.is_null());

    let sha_ptr = unsafe { getCCraneliftDSPFactorySHAKey(factory) };
    assert!(!sha_ptr.is_null());
    let looked_up = unsafe { getCCraneliftDSPFactoryFromSHAKey(sha_ptr.cast_const()) };
    assert_eq!(looked_up, factory);
    let instance = unsafe { createCCraneliftDSPInstance(factory) };
    assert!(!instance.is_null());

    let all_ptr = getAllCCraneliftDSPFactories();
    assert!(!all_ptr.is_null());
    let first = unsafe { *all_ptr };
    assert!(!first.is_null());

    unsafe {
        // free returned strings (outer array is intentionally not freed in scaffold).
        freeCMemory(first.cast());
        deleteAllCCraneliftDSPFactories();
        assert!(getCCraneliftDSPFactoryFromSHAKey(sha_ptr.cast_const()).is_null());
        freeCMemory(sha_ptr.cast());
    }
    // `factory`, `looked_up`, and `instance` were invalidated by clear.
}

#[test]
fn factory_cache_lifecycle_matches_reference_counted_cpp_contract() {
    let _guard = crate::test_serial_guard();
    super::clear_registered_foreign_functions();
    let name = c"cranelift_factory_lifecycle";
    let source = c"process = _;";
    let mut error = [0_i8; 4096];

    let mut create = || unsafe {
        createCCraneliftDSPFactoryFromString(
            name.as_ptr(),
            source.as_ptr(),
            0,
            std::ptr::null(),
            error.as_mut_ptr(),
            0,
        )
    };
    let first = create();
    let repeated = create();
    assert!(!first.is_null());
    assert_eq!(repeated, first);

    let sha = unsafe { CString::new((*first).sha_key.clone()).unwrap() };
    let looked_up = unsafe { getCCraneliftDSPFactoryFromSHAKey(sha.as_ptr()) };
    assert_eq!(looked_up, first);

    let instance = unsafe { createCCraneliftDSPInstance(first) };
    assert!(!instance.is_null());

    unsafe {
        assert!(!deleteCCraneliftDSPFactory(repeated));
        assert!(!deleteCCraneliftDSPFactory(looked_up));
        assert!(deleteCCraneliftDSPFactory(first));
        assert!(getCCraneliftDSPFactoryFromSHAKey(sha.as_ptr()).is_null());
    }
    // `instance` was owned by the cache and became invalid on final release.
}

#[test]
fn clif_bitcode_write_emits_v1_magic_header() {
    let _guard = crate::test_serial_guard();
    let name = c"bitcode";
    let src = c"process = _;";
    let mut err = [0_i8; 4096];
    let factory = unsafe {
        createCCraneliftDSPFactoryFromString(
            name.as_ptr(),
            src.as_ptr(),
            0,
            std::ptr::null(),
            err.as_mut_ptr(),
            1,
        )
    };
    assert!(!factory.is_null());

    let bitcode = unsafe { writeCCraneliftDSPFactoryToBitcode(factory) };
    assert!(!bitcode.is_null());
    let bitcode_s = unsafe { CStr::from_ptr(bitcode) }.to_str().unwrap();
    assert!(bitcode_s.starts_with("FAUST_CLIF_V1\n"));
    assert!(bitcode_s.contains("clif_func_count="));
    assert!(bitcode_s.contains("clif_func_name_0="));
    assert!(bitcode_s.contains("clif_func_body_0="));
    assert!(!bitcode_s.contains("clif_text=deferred"));

    unsafe {
        freeCMemory(bitcode.cast());
        assert!(deleteCCraneliftDSPFactory(factory));
    }
}

#[test]
fn clif_bitcode_file_write_emits_v1_magic_header() {
    let _guard = crate::test_serial_guard();
    let name = c"bitfile";
    let src = c"process = _;";
    let mut err = [0_i8; 4096];
    let factory = unsafe {
        createCCraneliftDSPFactoryFromString(
            name.as_ptr(),
            src.as_ptr(),
            0,
            std::ptr::null(),
            err.as_mut_ptr(),
            1,
        )
    };
    assert!(!factory.is_null());

    let path = std::env::temp_dir().join(format!(
        "faust-rs-cranelift-ffi-{}-{}.fbc.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let path_c = std::ffi::CString::new(path.as_os_str().to_string_lossy().as_bytes()).unwrap();

    let wrote = unsafe { writeCCraneliftDSPFactoryToBitcodeFile(factory, path_c.as_ptr()) };
    assert!(wrote);
    let text = std::fs::read_to_string(&path).expect("read written .clif");
    assert!(text.starts_with("FAUST_CLIF_V1\n"));
    assert!(text.contains("clif_func_count="));
    assert!(text.contains("clif_func_body_0="));

    let _ = std::fs::remove_file(&path);
    unsafe {
        assert!(deleteCCraneliftDSPFactory(factory));
    }
}

#[test]
fn clif_bitcode_roundtrip_in_memory_rebuilds_runnable_factory() {
    let _guard = crate::test_serial_guard();
    let name = c"clifroundtrip";
    let src = c"process = _;";
    let mut err = [0_i8; 4096];
    let factory = unsafe {
        createCCraneliftDSPFactoryFromString(
            name.as_ptr(),
            src.as_ptr(),
            0,
            std::ptr::null(),
            err.as_mut_ptr(),
            1,
        )
    };
    assert!(!factory.is_null());

    let payload = unsafe { writeCCraneliftDSPFactoryToBitcode(factory) };
    assert!(!payload.is_null());

    let restored =
        unsafe { readCCraneliftDSPFactoryFromBitcode(payload.cast_const(), err.as_mut_ptr()) };
    assert!(!restored.is_null());
    unsafe {
        assert!((*restored).compiled_jit.is_some());
        assert_eq!((*restored).num_inputs, (*factory).num_inputs);
        assert_eq!((*restored).num_outputs, (*factory).num_outputs);
        assert_eq!((*restored).sha_key, (*factory).sha_key);
        assert_eq!((*restored).compile_options, (*factory).compile_options);
        assert_eq!(restored, factory);
        freeCMemory(payload.cast());
        assert!(!deleteCCraneliftDSPFactory(factory));
        assert!(deleteCCraneliftDSPFactory(restored));
    }
}

#[test]
fn source_rebuild_accepts_legacy_allocation_dependent_sha() {
    let _guard = crate::test_serial_guard();
    super::clear_registered_foreign_functions();
    let name = "legacy_sha";
    let source = "process = _;";
    let argv = Vec::<String>::new();
    let opt_level = 1;

    let compiled =
        super::preflight_compile_source_to_cranelift(name, source, opt_level, &argv).unwrap();
    let legacy_sha = super::format_factory_sha_key(
        opt_level,
        &super::canonicalize_cache_identity_argv(&argv),
        &compiled.foreign_function_fingerprint,
        &fir::dump_fir(&compiled.fir.store, compiled.fir.module),
    );
    let current = super::build_scaffold_factory_common(
        super::FactoryBuildSpec {
            name,
            dsp_code: source,
            argv: &argv,
            opt_level,
            foreign_function_fingerprint: &compiled.foreign_function_fingerprint,
            source_is_faust: true,
        },
        &compiled.fir,
        Some(compiled.jit),
    )
    .unwrap();
    assert_ne!(current.sha_key, legacy_sha);
    let expected_compile_options = current.compile_options.clone();
    drop(current);

    let rebuilt = super::rebuild_factory_from_source(
        name,
        source,
        &argv,
        opt_level,
        &legacy_sha,
        &expected_compile_options,
    )
    .unwrap();
    assert_eq!(rebuilt.sha_key, legacy_sha);
}

#[test]
fn clif_bitcode_roundtrip_via_file_rebuilds_runnable_factory() {
    let _guard = crate::test_serial_guard();
    let name = c"cliffile";
    let src = c"process = _;";
    let mut err = [0_i8; 4096];
    let factory = unsafe {
        createCCraneliftDSPFactoryFromString(
            name.as_ptr(),
            src.as_ptr(),
            0,
            std::ptr::null(),
            err.as_mut_ptr(),
            1,
        )
    };
    assert!(!factory.is_null());

    let path = std::env::temp_dir().join(format!(
        "faust-rs-cranelift-ffi-clif-{}-{}.clif",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let path_c = std::ffi::CString::new(path.as_os_str().to_string_lossy().as_bytes()).unwrap();

    let wrote = unsafe { writeCCraneliftDSPFactoryToBitcodeFile(factory, path_c.as_ptr()) };
    assert!(wrote);
    let restored =
        unsafe { readCCraneliftDSPFactoryFromBitcodeFile(path_c.as_ptr(), err.as_mut_ptr()) };
    assert!(!restored.is_null());
    unsafe {
        assert!((*restored).compiled_jit.is_some());
        assert_eq!((*restored).num_inputs, (*factory).num_inputs);
        assert_eq!((*restored).num_outputs, (*factory).num_outputs);
        assert_eq!((*restored).sha_key, (*factory).sha_key);
        assert_eq!((*restored).compile_options, (*factory).compile_options);
        assert_eq!(restored, factory);
    }

    let _ = std::fs::remove_file(path);
    unsafe {
        assert!(!deleteCCraneliftDSPFactory(factory));
        assert!(deleteCCraneliftDSPFactory(restored));
    }
}

#[test]
fn source_backed_bitcode_read_rejects_invalid_format() {
    let _guard = crate::test_serial_guard();
    let bad = c"NOT_A_CRANELIFT_FORMAT";
    let mut err = [0_i8; 4096];
    let restored = unsafe { readCCraneliftDSPFactoryFromBitcode(bad.as_ptr(), err.as_mut_ptr()) };
    assert!(restored.is_null());
    let msg = unsafe { CStr::from_ptr(err.as_ptr()) }.to_str().unwrap();
    assert!(msg.contains("unsupported") || msg.contains("format"));
}

#[test]
fn shared_factory_builder_rejects_non_module_runtime_descriptor() {
    let _guard = crate::test_serial_guard();
    super::clear_registered_foreign_functions();
    let mut store = fir::FirStore::new();
    let bad_root = {
        let mut b = fir::FirBuilder::new(&mut store);
        b.int32(0)
    };
    let bad_fir = box_ffi::BoxFfiFirModule {
        store,
        module: bad_root,
        num_inputs: 0,
        num_outputs: 0,
    };
    let result = super::build_scaffold_factory_common(
        super::FactoryBuildSpec {
            name: "dsp",
            dsp_code: "process = _;",
            argv: &[],
            opt_level: 1,
            foreign_function_fingerprint: "",
            source_is_faust: true,
        },
        &bad_fir,
        None,
    );
    assert!(
        result.is_err(),
        "builder must fail on non-module FIR runtime descriptors"
    );
}

#[test]
fn selected_runtime_corpus_cases_lower_compute_body() {
    let _guard = crate::test_serial_guard();
    super::clear_registered_foreign_functions();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root");
    // `rep_38_sine_phasor` now lowers through fixed-size FIR delay-line
    // arrays (`fDelay*`). The current Cranelift bring-up contract still
    // rejects DSP struct array fields, so keep that fixture out of the
    // selected lowered-body subset until Array struct fields are supported.
    let cases = [
        "tests/corpus/rep_01_passthrough.dsp",
        "tests/corpus/rep_02_gain_bias.dsp",
        "tests/corpus/rep_03_stereo_mix.dsp",
        "tests/corpus/rep_07_nonlinear_clip.dsp",
    ];

    for rel in cases {
        let mut err = [0_i8; 4096];
        let path = root.join(rel);
        let c_path =
            std::ffi::CString::new(path.to_string_lossy().as_bytes()).expect("path CString");
        let factory = unsafe {
            createCCraneliftDSPFactoryFromFile(
                c_path.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                1,
            )
        };
        assert!(
            !factory.is_null(),
            "factory creation failed for {rel}: {}",
            unsafe { CStr::from_ptr(err.as_ptr()) }
                .to_string_lossy()
                .into_owned()
        );
        unsafe {
            assert!(
                (*factory).compute_body_lowered,
                "Cranelift fallback used for selected corpus case {rel}"
            );
            assert!(deleteCCraneliftDSPFactory(factory));
        }
    }
}

#[test]
fn clif_save_restore_selected_corpus_cases() {
    let _guard = crate::test_serial_guard();
    super::clear_registered_foreign_functions();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root");
    // Keep the same reduced subset as `selected_runtime_corpus_cases_lower_compute_body`.
    let cases = [
        "tests/corpus/rep_01_passthrough.dsp",
        "tests/corpus/rep_02_gain_bias.dsp",
        "tests/corpus/rep_03_stereo_mix.dsp",
        "tests/corpus/rep_07_nonlinear_clip.dsp",
    ];

    for rel in cases {
        let mut err = [0_i8; 4096];
        let path = root.join(rel);
        let c_path =
            std::ffi::CString::new(path.to_string_lossy().as_bytes()).expect("path CString");
        let factory = unsafe {
            createCCraneliftDSPFactoryFromFile(
                c_path.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                1,
            )
        };
        assert!(
            !factory.is_null(),
            "create from file failed for {rel}: {}",
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
        );

        let payload = unsafe { writeCCraneliftDSPFactoryToBitcode(factory) };
        assert!(!payload.is_null(), "write bitcode failed for {rel}");
        let restored =
            unsafe { readCCraneliftDSPFactoryFromBitcode(payload.cast_const(), err.as_mut_ptr()) };
        assert!(
            !restored.is_null(),
            "restore from bitcode failed for {rel}: {}",
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
        );

        unsafe {
            assert!(
                (*factory).compiled_jit.is_some(),
                "missing original jit for {rel}"
            );
            assert!(
                (*restored).compiled_jit.is_some(),
                "missing restored jit for {rel}"
            );
            assert_eq!(
                (*restored).sha_key,
                (*factory).sha_key,
                "sha mismatch for {rel}"
            );
            assert_eq!(
                (*restored).compile_options,
                (*factory).compile_options,
                "compile_options mismatch for {rel}"
            );
            assert_eq!(
                restored, factory,
                "restore must coalesce with the cached factory for {rel}"
            );
            freeCMemory(payload.cast());
            assert!(!deleteCCraneliftDSPFactory(factory));
            assert!(deleteCCraneliftDSPFactory(restored));
        }
    }
}

#[test]
fn boxes_and_signals_constructor_match_string_constructor_sha() {
    let _guard = crate::test_serial_guard();
    super::clear_registered_foreign_functions();
    box_ffi::createLibContext();
    let box_root = box_ffi::CboxWire();
    assert!(!box_root.is_null());

    let name = c"same";
    let src = c"process = _;";
    let mut err = [0_i8; 4096];

    let from_box = unsafe {
        createCCraneliftDSPFactoryFromBoxes(
            name.as_ptr(),
            box_root,
            0,
            std::ptr::null(),
            err.as_mut_ptr(),
            1,
        )
    };
    assert!(!from_box.is_null());
    let signals = unsafe { box_ffi::CboxesToSignals(box_root, err.as_mut_ptr()) };
    assert!(!signals.is_null());
    let from_signals = unsafe {
        createCCraneliftDSPFactoryFromSignals(
            name.as_ptr(),
            signals.cast(),
            0,
            std::ptr::null(),
            err.as_mut_ptr(),
            1,
        )
    };
    assert!(!from_signals.is_null());
    let from_string = unsafe {
        createCCraneliftDSPFactoryFromString(
            name.as_ptr(),
            src.as_ptr(),
            0,
            std::ptr::null(),
            err.as_mut_ptr(),
            1,
        )
    };
    assert!(!from_string.is_null());

    let sha_box_ptr = unsafe { getCCraneliftDSPFactorySHAKey(from_box) };
    let sha_signals_ptr = unsafe { getCCraneliftDSPFactorySHAKey(from_signals) };
    let sha_string_ptr = unsafe { getCCraneliftDSPFactorySHAKey(from_string) };
    let sha_box = unsafe { CStr::from_ptr(sha_box_ptr) }
        .to_string_lossy()
        .into_owned();
    let sha_signals = unsafe { CStr::from_ptr(sha_signals_ptr) }
        .to_string_lossy()
        .into_owned();
    let sha_string = unsafe { CStr::from_ptr(sha_string_ptr) }
        .to_string_lossy()
        .into_owned();
    assert_eq!(sha_box, sha_signals);
    assert_eq!(sha_box, sha_string);
    assert_eq!(from_box, from_signals);
    assert_eq!(from_box, from_string);

    unsafe {
        freeCMemory(sha_box_ptr.cast());
        freeCMemory(sha_signals_ptr.cast());
        freeCMemory(sha_string_ptr.cast());
        box_ffi::freeCMemory(signals.cast());
        assert!(!deleteCCraneliftDSPFactory(from_box));
        assert!(!deleteCCraneliftDSPFactory(from_signals));
        assert!(deleteCCraneliftDSPFactory(from_string));
    }
    box_ffi::destroyLibContext();
}

#[test]
fn registered_foreign_function_is_used_for_factory_build() {
    let _guard = crate::test_serial_guard();
    super::clear_registered_foreign_functions();
    let name = c"foreign_fun";
    let src = c"process = ffunction(float ffi_test_foreign_gain(float), <math.h>, \"\");";
    let mut err = [0_i8; 4096];

    unsafe {
        registerCCraneliftForeignFunction(
            c"ffi_test_foreign_gain".as_ptr(),
            (ffi_test_foreign_gain as *const ()).cast_mut().cast(),
        );
    }

    let factory = unsafe {
        createCCraneliftDSPFactoryFromString(
            name.as_ptr(),
            src.as_ptr(),
            0,
            std::ptr::null(),
            err.as_mut_ptr(),
            1,
        )
    };
    assert!(
        !factory.is_null(),
        "factory creation failed: {}",
        unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
    );

    unsafe {
        assert!((*factory).compute_body_lowered);
        assert!(deleteCCraneliftDSPFactory(factory));
    }
    super::clear_registered_foreign_functions();
}

#[test]
fn unregister_foreign_function_removes_future_binding() {
    let _guard = crate::test_serial_guard();
    super::clear_registered_foreign_functions();
    let name = c"foreign_fun_unreg";
    let src = c"process = ffunction(float ffi_test_foreign_gain(float), <math.h>, \"\");";
    let mut err = [0_i8; 4096];

    unsafe {
        registerCCraneliftForeignFunction(
            c"ffi_test_foreign_gain".as_ptr(),
            (ffi_test_foreign_gain as *const ()).cast_mut().cast(),
        );
        unregisterCCraneliftForeignFunction(c"ffi_test_foreign_gain".as_ptr());
    }

    let factory = unsafe {
        createCCraneliftDSPFactoryFromString(
            name.as_ptr(),
            src.as_ptr(),
            0,
            std::ptr::null(),
            err.as_mut_ptr(),
            1,
        )
    };
    assert!(
        !factory.is_null(),
        "factory creation failed after unregister: {}",
        unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
    );

    unsafe {
        assert!(
            !(*factory).compute_body_lowered,
            "unregistered foreign function should fall back to the stub path"
        );
        assert!(deleteCCraneliftDSPFactory(factory));
    }
    super::clear_registered_foreign_functions();
}

#[test]
fn clear_foreign_functions_removes_all_future_bindings() {
    let _guard = crate::test_serial_guard();
    super::clear_registered_foreign_functions();
    let name = c"foreign_fun_clear";
    let src = c"process = ffunction(float ffi_test_foreign_gain(float), <math.h>, \"\");";
    let mut err = [0_i8; 4096];

    unsafe {
        registerCCraneliftForeignFunction(
            c"ffi_test_foreign_gain".as_ptr(),
            (ffi_test_foreign_gain as *const ()).cast_mut().cast(),
        );
    }
    clearCCraneliftForeignFunctions();

    let factory = unsafe {
        createCCraneliftDSPFactoryFromString(
            name.as_ptr(),
            src.as_ptr(),
            0,
            std::ptr::null(),
            err.as_mut_ptr(),
            1,
        )
    };
    assert!(
        !factory.is_null(),
        "factory creation failed after clear: {}",
        unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
    );

    unsafe {
        assert!(
            !(*factory).compute_body_lowered,
            "cleared foreign functions should fall back to the stub path"
        );
        assert!(deleteCCraneliftDSPFactory(factory));
    }
    super::clear_registered_foreign_functions();
}

#[test]
fn cpp_header_exposes_cranelift_foreign_function_api() {
    let header = std::fs::read_to_string(format!(
        "{}/include/cranelift-dsp.h",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("cranelift C++ header should be readable");

    assert!(header.contains("inline void registerCraneliftForeignFunction("));
    assert!(
        header.contains("inline void unregisterCraneliftForeignFunction(const std::string& name)")
    );
    assert!(header.contains("inline void clearCraneliftForeignFunctions()"));
    assert!(header.contains("setCCraneliftMemoryManager"));
    assert!(header.contains("return memory_manager_;"));
    assert!(!header.contains("setMemoryManager(dsp_memory_manager* /*manager*/)"));
}

#[test]
fn cranelift_header_mirrors_the_canonical_memory_manager_abi() {
    assert_eq!(
        include_str!("../../include/faust-memory-manager.h"),
        include_str!("../../../ffi-common/include/faust-memory-manager.h")
    );
    assert!(
        include_str!("../../include/cranelift-dsp-c.h")
            .contains("#include \"faust-memory-manager.h\"")
    );
}

/// The FIR the string constructor compiles `source` to under `argv`.
fn fir_under(source: &str, argv: &[&str]) -> String {
    let argv: Vec<String> = argv.iter().map(|arg| (*arg).to_owned()).collect();
    let compiled = super::preflight_compile_source_to_cranelift("options", source, 0, &argv)
        .unwrap_or_else(|error| panic!("{argv:?}: {error}"));
    fir::dump_fir(&compiled.fir.store, compiled.fir.module)
}

#[test]
fn process_name_delay_and_table_options_reach_the_compiler() {
    let _guard = crate::test_serial_guard();
    // Each option against the same program without it: the FIR must change,
    // or the option was dropped on the way (as `-pn` was, silently).
    let cases: [(&str, &[&str]); 5] = [
        ("process = 1; other = 2;", &["-pn", "other"]),
        ("process = 1; other = 2;", &["--process-name", "other"]),
        // a 3-sample delay is a shifted copy up to `-mcd 16`, a ring below
        ("process = @(3);", &["-mcd", "0"]),
        // a 100-sample delay is a power-of-two ring unless `-dlt` is lower
        ("process = @(100);", &["-dlt", "50"]),
        // an index of unknown range is clamped unless `-ct 0`
        ("process = rdtable(8, 1.0, int(_));", &["-ct", "0"]),
    ];
    for (source, argv) in cases {
        assert_ne!(
            fir_under(source, &[]),
            fir_under(source, argv),
            "{argv:?} did not change the FIR of `{source}`"
        );
    }
    // and the defaults spelled out change nothing
    for (source, argv) in [
        ("process = @(3);", &["-mcd", "16"][..]),
        ("process = rdtable(8, 1.0, int(_));", &["-ct", "1"]),
        ("process = 1; other = 2;", &["-pn", "process"]),
    ] {
        assert_eq!(fir_under(source, &[]), fir_under(source, argv), "{argv:?}");
    }
}
