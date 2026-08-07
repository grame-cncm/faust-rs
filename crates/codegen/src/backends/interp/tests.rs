use super::*;
use fir::{FirBuilder, FirType, NamedType};

fn make_minimal_legacy_like_module() -> (fir::FirStore, fir::FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let label = b.label("legacy bridge compute stub");
    let body = b.block(&[label]);
    let ff_ptr_ptr = FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat))));
    let compute_type = FirType::Fun {
        args: vec![
            FirType::Ptr(Box::new(FirType::Obj)),
            FirType::Int32,
            ff_ptr_ptr.clone(),
            ff_ptr_ptr,
        ],
        ret: Box::new(FirType::Void),
    };
    let compute_args = [
        NamedType {
            name: "dsp".into(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".into(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".into(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".into(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun("compute", compute_type, &compute_args, Some(body), false);
    let dsp_struct = b.block(&[]);
    let globals = b.block(&[]);
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        0,
        "legacy_like",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

#[test]
fn generate_interp_module_reserves_sr_and_count_slots_when_missing() {
    let (store, module) = make_minimal_legacy_like_module();
    let factory = generate_interp_module::<f32>(
        &store,
        module,
        &InterpOptions {
            opt_level: 0,
            module_name: None,
            ..InterpOptions::default()
        },
    )
    .expect("minimal legacy-like module should compile to interp factory");

    assert!(factory.int_heap_size >= 2);
    assert!(factory.sr_offset >= 0);
    assert!(factory.count_offset >= 0);
    assert!(factory.sr_offset < factory.int_heap_size);
    assert!(factory.count_offset < factory.int_heap_size);
    assert_ne!(factory.sr_offset, factory.count_offset);
}
