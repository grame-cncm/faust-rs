//! Differential runtime tests (`interp` vs `cranelift`) for smoke corpus cases.
//!
//! These tests provide backend-alignment coverage for the Cranelift bring-up:
//! - numeric output comparison against interpreter backend on selected cases,
//! - UI/meta callback smoke validation on a UI-focused runtime case.

#[cfg(test)]
mod tests {
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::io::BufReader;
    use std::path::{Path, PathBuf};
    use std::thread;

    use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
    use compiler::{Compiler, SignalFirLane};

    use crate::factory::{createCCraneliftDSPFactoryFromFile, deleteCCraneliftDSPFactory};
    use crate::instance::{
        buildUserInterfaceCCraneliftDSPInstance, computeCCraneliftDSPInstance,
        createCCraneliftDSPInstance, deleteCCraneliftDSPInstance,
        getNumInputsCCraneliftDSPInstance, getNumOutputsCCraneliftDSPInstance,
        initCCraneliftDSPInstance, metadataCCraneliftDSPInstance,
    };
    use crate::types::{FaustFloat, MetaGlue, UIGlue};

    const SAMPLE_RATE: usize = 48_000;
    const BLOCK_SIZE: usize = 64;
    const NUM_BLOCKS: usize = 8;
    const ABS_TOL: f32 = 5e-4;
    const REL_TOL: f32 = 5e-4;
    /// The faustlibraries checkout used when `FAUST_RS_FAUSTLIBRARIES_ROOT` is
    /// unset: `faustlibraries` next to the directory that holds this repository.
    fn default_faustlibraries_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(4)
            .map_or_else(std::path::PathBuf::new, |dir| dir.join("faustlibraries"))
    }

    fn c_int_arity_to_usize(value: i32, label: &str) -> Result<usize, String> {
        usize::try_from(value).map_err(|_| format!("invalid negative {label}: {value}"))
    }

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root")
    }

    fn faustlibraries_root() -> Option<PathBuf> {
        std::env::var_os("FAUST_RS_FAUSTLIBRARIES_ROOT")
            .map(PathBuf::from)
            .or_else(|| {
                let default = default_faustlibraries_root();
                default.exists().then_some(default)
            })
    }

    fn run_with_large_stack<T>(f: impl FnOnce() -> T + Send + 'static) -> T
    where
        T: Send + 'static,
    {
        thread::Builder::new()
            .name("cranelift-runtime-diff".to_owned())
            .stack_size(64 * 1024 * 1024)
            .spawn(f)
            .expect("spawn cranelift runtime diff worker thread")
            .join()
            .expect("cranelift runtime diff worker thread should finish")
    }

    fn generate_impulse_inputs(num_inputs: usize, total_samples: usize) -> Vec<Vec<f32>> {
        let mut channels = vec![vec![0.0f32; total_samples]; num_inputs];
        for (ch_idx, channel) in channels.iter_mut().enumerate() {
            if let Some(first) = channel.first_mut() {
                *first = 1.0f32 + ch_idx as f32;
            }
        }
        channels
    }

    fn run_interp_outputs(
        case: &Path,
        import_root: Option<&Path>,
    ) -> Result<Vec<Vec<f32>>, String> {
        let compiler = Compiler::new();
        let search_paths = import_root
            .map(|root| vec![root.to_path_buf()])
            .unwrap_or_default();
        let fbc = compiler
            .compile_file_to_interp_with_lane(
                case,
                &search_paths,
                &InterpOptions::default(),
                SignalFirLane::TransformFastLane,
            )
            .map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(fbc.as_bytes());
        let mut factory = read_fbc::<f32>(&mut reader).map_err(|e| e.to_string())?;
        let num_inputs = c_int_arity_to_usize(factory.num_inputs, "interp input arity")?;
        let num_outputs = c_int_arity_to_usize(factory.num_outputs, "interp output arity")?;
        let mut instance = FbcDspInstance::new(&mut factory);
        instance.init(SAMPLE_RATE as i32);

        let total_samples = BLOCK_SIZE * NUM_BLOCKS;
        let input_channels = generate_impulse_inputs(num_inputs, total_samples);
        let mut output_channels = vec![vec![0.0f32; total_samples]; num_outputs];

        for block_idx in 0..NUM_BLOCKS {
            let start = block_idx * BLOCK_SIZE;
            let end = start + BLOCK_SIZE;
            let input_refs: Vec<&[f32]> = input_channels.iter().map(|ch| &ch[start..end]).collect();
            let mut output_refs: Vec<&mut [f32]> = output_channels
                .iter_mut()
                .map(|ch| &mut ch[start..end])
                .collect();
            instance
                .try_compute(BLOCK_SIZE as i32, &input_refs, &mut output_refs)
                .map_err(|e| e.to_string())?;
        }
        Ok(output_channels)
    }

    fn run_cranelift_outputs(
        case: &Path,
        import_root: Option<&Path>,
    ) -> Result<Vec<Vec<f32>>, String> {
        let c_path = CString::new(case.to_string_lossy().as_bytes())
            .map_err(|e| format!("case path is not valid C string: {e}"))?;
        let argv_storage = import_root
            .map(|root| {
                vec![
                    CString::new("-I").expect("CString -I"),
                    CString::new(root.to_string_lossy().as_bytes()).expect("CString import root"),
                ]
            })
            .unwrap_or_default();
        let argv_ptrs = argv_storage
            .iter()
            .map(|arg| arg.as_ptr())
            .collect::<Vec<_>>();
        let mut err = [0_i8; 4096];
        let factory = unsafe {
            createCCraneliftDSPFactoryFromFile(
                c_path.as_ptr(),
                i32::try_from(argv_ptrs.len()).expect("argv length fits in c_int"),
                if argv_ptrs.is_empty() {
                    std::ptr::null()
                } else {
                    argv_ptrs.as_ptr()
                },
                err.as_mut_ptr(),
                1,
            )
        };
        if factory.is_null() {
            return Err(format!(
                "Cranelift factory creation failed: {}",
                unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
            ));
        }

        let dsp = unsafe { createCCraneliftDSPInstance(factory) };
        if dsp.is_null() {
            unsafe {
                let _ = deleteCCraneliftDSPFactory(factory);
            }
            return Err("Cranelift instance creation failed".to_owned());
        }
        unsafe { initCCraneliftDSPInstance(dsp, SAMPLE_RATE as i32) };

        let num_inputs = c_int_arity_to_usize(
            unsafe { getNumInputsCCraneliftDSPInstance(dsp) },
            "input arity",
        )?;
        let num_outputs = c_int_arity_to_usize(
            unsafe { getNumOutputsCCraneliftDSPInstance(dsp) },
            "output arity",
        )?;
        let total_samples = BLOCK_SIZE * NUM_BLOCKS;
        let mut input_channels = generate_impulse_inputs(num_inputs, total_samples);
        let mut output_channels = vec![vec![0.0f32; total_samples]; num_outputs];

        for block_idx in 0..NUM_BLOCKS {
            let start = block_idx * BLOCK_SIZE;
            let end = start + BLOCK_SIZE;

            let mut input_ptrs: Vec<*mut FaustFloat> = Vec::with_capacity(num_inputs);
            for channel in &mut input_channels {
                input_ptrs.push(channel[start..end].as_mut_ptr());
            }

            let mut output_ptrs: Vec<*mut FaustFloat> = Vec::with_capacity(num_outputs);
            for channel in &mut output_channels {
                output_ptrs.push(channel[start..end].as_mut_ptr());
            }

            unsafe {
                computeCCraneliftDSPInstance(
                    dsp,
                    BLOCK_SIZE as i32,
                    input_ptrs.as_mut_ptr(),
                    output_ptrs.as_mut_ptr(),
                )
            };
        }

        unsafe {
            deleteCCraneliftDSPInstance(dsp);
            let _ = deleteCCraneliftDSPFactory(factory);
        }
        Ok(output_channels)
    }

    fn assert_outputs_close(case: &Path, interp: &[Vec<f32>], cranelift: &[Vec<f32>]) {
        assert_eq!(
            interp.len(),
            cranelift.len(),
            "backend mismatch for {}: interp outputs={} vs cranelift outputs={}",
            case.display(),
            interp.len(),
            cranelift.len()
        );
        for (ch_idx, (left, right)) in interp.iter().zip(cranelift.iter()).enumerate() {
            assert_eq!(
                left.len(),
                right.len(),
                "backend mismatch for {}: channel {} length differs (interp={}, cranelift={})",
                case.display(),
                ch_idx,
                left.len(),
                right.len()
            );
            for (sample_idx, (&l, &r)) in left.iter().zip(right.iter()).enumerate() {
                // NaN-vs-NaN is a valid match (both backends agree on
                // undefined arithmetic), skip before IEEE 754 NaN != NaN
                // would produce a spurious failure.
                if l.is_nan() && r.is_nan() {
                    continue;
                }
                let delta = (l - r).abs();
                let allowed = ABS_TOL.max(REL_TOL * l.abs().max(r.abs()));
                assert!(
                    delta <= allowed,
                    "backend mismatch for {} at ch={} sample={}: interp={} cranelift={} delta={} allowed={}",
                    case.display(),
                    ch_idx,
                    sample_idx,
                    l,
                    r,
                    delta,
                    allowed
                );
            }
        }
    }

    #[derive(Default)]
    struct UiCounter {
        calls: usize,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct UiDeclareEvent {
        zone_is_null: bool,
        key: String,
        value: String,
    }

    unsafe extern "C" fn count_ui_slider(
        ui: *mut c_void,
        _label: *const c_char,
        _zone: *mut FaustFloat,
        _init: FaustFloat,
        _min: FaustFloat,
        _max: FaustFloat,
        _step: FaustFloat,
    ) {
        unsafe {
            let counter = &mut *(ui.cast::<UiCounter>());
            counter.calls += 1;
        }
    }

    unsafe extern "C" fn count_ui_declare(
        ui: *mut c_void,
        _zone: *mut FaustFloat,
        _key: *const c_char,
        _value: *const c_char,
    ) {
        unsafe {
            let counter = &mut *(ui.cast::<UiCounter>());
            counter.calls += 1;
        }
    }

    unsafe extern "C" fn count_meta(meta: *mut c_void, _key: *const c_char, _value: *const c_char) {
        unsafe {
            let count = &mut *(meta.cast::<usize>());
            *count += 1;
        }
    }

    unsafe extern "C" fn capture_ui_declare(
        ui: *mut c_void,
        zone: *mut FaustFloat,
        key: *const c_char,
        value: *const c_char,
    ) {
        unsafe {
            let events = &mut *(ui.cast::<Vec<UiDeclareEvent>>());
            events.push(UiDeclareEvent {
                zone_is_null: zone.is_null(),
                key: CStr::from_ptr(key).to_string_lossy().into_owned(),
                value: CStr::from_ptr(value).to_string_lossy().into_owned(),
            });
        }
    }

    unsafe extern "C" fn capture_meta_entry(
        meta: *mut c_void,
        key: *const c_char,
        value: *const c_char,
    ) {
        unsafe {
            let entries = &mut *(meta.cast::<Vec<(String, String)>>());
            entries.push((
                CStr::from_ptr(key).to_string_lossy().into_owned(),
                CStr::from_ptr(value).to_string_lossy().into_owned(),
            ));
        }
    }

    #[test]
    fn cranelift_interp_runtime_diff_smoke_corpus() {
        run_with_large_stack(|| {
            let _guard = crate::test_serial_guard();
            let root = workspace_root();
            let faustlib_root = faustlibraries_root();
            // This corpus drives deep compiler recursion during the interpreter
            // side of the differential run, so execute it on an explicitly
            // larger stack instead of depending on the default test-thread size.
            //
            // `rep_38_sine_phasor` remains excluded from this strict
            // differential smoke set until Cranelift stateful-loop parity is
            // completed.
            let mut cases = vec![
                root.join("tests/corpus/rep_01_passthrough.dsp"),
                root.join("tests/corpus/rep_07_nonlinear_clip.dsp"),
                root.join("tests/corpus/rep_60_counter_rem.dsp"),
                root.join("tests/corpus/rep_62_select2_trigger.dsp"),
                root.join("tests/corpus/rep_63_rwtable.dsp"),
                root.join("tests/corpus/rep_65_fabs_trigger.dsp"),
            ];
            if faustlib_root.is_some() {
                cases.push(root.join("tests/corpus/rep_61_fmin_sr.dsp"));
                cases.push(root.join("tests/corpus/rep_64_dynamic_rem.dsp"));
            } else {
                eprintln!(
                    "Skipping stdlib-dependent Cranelift smoke cases: stdfaust.lib unavailable and FAUST_RS_FAUSTLIBRARIES_ROOT is unset"
                );
            }
            for case in &cases {
                let interp =
                    run_interp_outputs(case, faustlib_root.as_deref()).unwrap_or_else(|e| {
                        panic!("interp backend runtime failed for {}: {e}", case.display())
                    });
                let cranelift = run_cranelift_outputs(case, faustlib_root.as_deref())
                    .unwrap_or_else(|e| {
                        panic!(
                            "cranelift backend runtime failed for {}: {e}",
                            case.display()
                        )
                    });
                assert_outputs_close(case, &interp, &cranelift);
            }
        });
    }

    #[test]
    fn cranelift_ui_meta_callback_smoke_path() {
        let _guard = crate::test_serial_guard();
        let case = workspace_root().join("tests/runtime_corpus/trace_09_ui_slider.dsp");
        let c_path = CString::new(case.to_string_lossy().as_bytes()).expect("path CString");
        let mut err = [0_i8; 4096];
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
            "Cranelift factory creation failed for {}: {}",
            case.display(),
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
        );

        let dsp = unsafe { createCCraneliftDSPInstance(factory) };
        assert!(!dsp.is_null(), "Cranelift instance creation failed");
        unsafe { initCCraneliftDSPInstance(dsp, SAMPLE_RATE as i32) };

        let mut ui_counter = UiCounter::default();
        let mut ui = UIGlue {
            ui_interface: (&mut ui_counter as *mut UiCounter).cast::<c_void>(),
            open_tab_box: None,
            open_horizontal_box: None,
            open_vertical_box: None,
            close_box: None,
            add_button: None,
            add_check_button: None,
            add_vertical_slider: Some(count_ui_slider),
            add_horizontal_slider: Some(count_ui_slider),
            add_num_entry: Some(count_ui_slider),
            add_horizontal_bargraph: None,
            add_vertical_bargraph: None,
            add_soundfile: None,
            declare: Some(count_ui_declare),
        };
        unsafe { buildUserInterfaceCCraneliftDSPInstance(dsp, &mut ui) };
        assert!(
            ui_counter.calls > 0,
            "expected UI callbacks for {}",
            case.display()
        );

        let mut meta_events = 0usize;
        let mut meta = MetaGlue {
            meta_interface: (&mut meta_events as *mut usize).cast::<c_void>(),
            declare: Some(count_meta),
        };
        unsafe { metadataCCraneliftDSPInstance(dsp, &mut meta) };
        assert!(
            meta_events > 0,
            "expected metadata callbacks for {}",
            case.display()
        );

        unsafe {
            deleteCCraneliftDSPInstance(dsp);
            let _ = deleteCCraneliftDSPFactory(factory);
        }
    }

    #[test]
    fn cranelift_replays_ui_declares_separately_from_metadata_callback() {
        let _guard = crate::test_serial_guard();
        let case = workspace_root().join("tests/corpus/rep_56_noise_smoo_slider.dsp");
        let c_path = CString::new(case.to_string_lossy().as_bytes()).expect("path CString");
        let mut err = [0_i8; 4096];
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
            "Cranelift factory creation failed for {}: {}",
            case.display(),
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
        );

        let dsp = unsafe { createCCraneliftDSPInstance(factory) };
        assert!(!dsp.is_null(), "Cranelift instance creation failed");
        unsafe { initCCraneliftDSPInstance(dsp, SAMPLE_RATE as i32) };

        let mut ui_declares = Vec::<UiDeclareEvent>::new();
        let mut ui = UIGlue {
            ui_interface: (&mut ui_declares as *mut Vec<UiDeclareEvent>).cast::<c_void>(),
            open_tab_box: None,
            open_horizontal_box: None,
            open_vertical_box: None,
            close_box: None,
            add_button: None,
            add_check_button: None,
            add_vertical_slider: None,
            add_horizontal_slider: None,
            add_num_entry: None,
            add_horizontal_bargraph: None,
            add_vertical_bargraph: None,
            add_soundfile: None,
            declare: Some(capture_ui_declare),
        };
        unsafe { buildUserInterfaceCCraneliftDSPInstance(dsp, &mut ui) };
        assert!(
            ui_declares
                .iter()
                .any(|event| event.key == "style" && event.value == "knob" && !event.zone_is_null),
            "expected style=knob UI declare with a control zone for {} but saw {ui_declares:?}",
            case.display()
        );

        let mut meta_entries = Vec::<(String, String)>::new();
        let mut meta = MetaGlue {
            meta_interface: (&mut meta_entries as *mut Vec<(String, String)>).cast::<c_void>(),
            declare: Some(capture_meta_entry),
        };
        unsafe { metadataCCraneliftDSPInstance(dsp, &mut meta) };
        assert!(
            !meta_entries
                .iter()
                .any(|(key, value)| key == "style" && value == "knob"),
            "UI-only metadata should not leak into metadata() for {} but saw {meta_entries:?}",
            case.display()
        );

        unsafe {
            deleteCCraneliftDSPInstance(dsp);
            let _ = deleteCCraneliftDSPFactory(factory);
        }
    }
}
