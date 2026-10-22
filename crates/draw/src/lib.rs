#![forbid(unsafe_code)]

//! SVG block-diagram generation for Faust DSP programs.
//!
//! This crate ports the C++ `compiler/draw/` module to Rust, providing the
//! `-svg` compilation flag that produces visual block diagrams of a Faust
//! program's structure.
//!
//! # Architecture
//!
//! ```text
//! draw_diagrams(arena, root, name, title, config, def_names)
//!   for each queued diagram (root, then any folded-out sub-diagrams):
//!     └── translate::generate_folded_inside(arena, box_id, config, &mut fold_state)
//!           └── schemas::{Block,Seq,Par,…}  → Box<dyn Schema>   (recursive; folds
//!               named sub-diagrams above `config.fold_complexity` into new
//!               queue entries instead of inlining them)
//!     └── translate::make_top_schema(inner, diagram_name, back_link)  (title + margin)
//!     └── SvgDevice::new(file, width, height, config)
//!     └── schema.place(0, 0, LeftRight)
//!     └── schema.draw(dev)
//!     └── TraitCollector::draw(dev)                       (wires on top)
//! ```
//!
//! # C++ source mapping
//!
//! | Rust module | C++ source |
//! |-------------|-----------|
//! | `error` | — (replaces `faustexception`) |
//! | `device` | `device/device.h`, `device/SVGDev.h/cpp` |
//! | `schema` | `schema/schema.h`, `schema/collector.cpp` |
//! | `schemas::block` | `schema/blockSchema.h/cpp`, `schema/inverterSchema.h/cpp` |
//! | `schemas::cable` | `schema/cableSchema.h/cpp`, `schema/cutSchema.h/cpp`, `schema/connectorSchema.h/cpp` |
//! | `schemas::seq` | `schema/seqSchema.h/cpp` |
//! | `schemas::par` | `schema/parSchema.h/cpp` |
//! | `schemas::merge` | `schema/mergeSchema.h/cpp` |
//! | `schemas::split` | `schema/splitSchema.h/cpp` |
//! | `schemas::rec` | `schema/recSchema.h/cpp` |
//! | `schemas::composed` | `schema/topSchema.h/cpp`, `schema/decorateSchema.h/cpp`, `schema/enlargedSchema.h/cpp` |
//! | `schemas::route` | `schema/routeSchema.h/cpp` |
//! | `schemas::multirate` | `schema/ondemandSchema.h/cpp`, `schema/downsamplingSchema.h/cpp`, `schema/upsamplingSchema.h/cpp` |
//! | `translate` | `drawschema.cpp` |

pub mod device;
pub mod error;
pub mod schema;
pub mod schemas;
pub mod translate;

pub use error::DrawError;
pub use schema::{
    COLOR_INV, COLOR_LINK, COLOR_NORMAL, COLOR_NUM, COLOR_SLOT, COLOR_UI, D_HORZ, D_LETTER, D_VERT,
    D_WIRE, Orientation, Placement, Point, Schema, Trait, TraitCollector,
};
pub use translate::FileNaming;

pub const CRATE_NAME: &str = "draw";

#[must_use]
/// Returns the stable crate identifier.
pub fn crate_id() -> &'static str {
    CRATE_NAME
}

// ─── DrawConfig ───────────────────────────────────────────────────────────────

/// Visual and layout options for SVG block-diagram generation.
///
/// Matches the set of C++ globals consumed by the draw module.
/// All fields have documented defaults matching the reference compiler.
///
/// C++ references: `global.hh` fields `gShadowBlur`, `gScaledSVG`,
/// `gDrawRouteFrame`, `gMaxNameSize`.
#[derive(Clone, Debug)]
pub struct DrawConfig {
    /// Add a Gaussian drop-shadow filter to each box rectangle.
    ///
    /// Emits a `<defs><filter>` block in the SVG header and applies
    /// `filter:url(#filter)` to shadow rects.
    ///
    /// C++ global: `gShadowBlur` (default false). CLI: `-blur` / `--shadow-blur`.
    pub shadow_blur: bool,

    /// Emit a viewBox-only SVG header (no fixed `width=` / `height=` mm attributes).
    ///
    /// Makes the SVG responsive — scales freely to container size.
    ///
    /// C++ global: `gScaledSVG` (default false). CLI: `-sc` / `--scaled-svg`.
    pub scaled_svg: bool,

    /// Draw a solid rectangle frame around route boxes instead of cable stubs.
    ///
    /// Without this flag (the default), a route box draws nothing at all —
    /// it is purely structural, and only its wire traits carry the signal.
    /// With this flag, it draws a filled background rectangle, an orientation
    /// mark, and input arrows.
    ///
    /// C++ global: `gDrawRouteFrame` (default false). CLI: `-drf` / `--draw-route-frame`.
    pub draw_route_frame: bool,

    /// Maximum character length for block labels; longer names are truncated.
    ///
    /// Truncation keeps the first third and last third of the name, joined by `"..."`.
    ///
    /// C++ global: `gMaxNameSize` (default 40). CLI: `-mns N` / `--max-name-size N`.
    pub max_name_size: usize,

    /// Fold diagrams whose total `box_complexity` exceeds this threshold into
    /// separate SVG files with clickable back-links.
    ///
    /// When 0, folding is disabled (single-file output).
    ///
    /// C++ global: `gFoldThreshold` (default 25). CLI: `-f N` / `--fold N`.
    pub fold_threshold: usize,

    /// Minimum per-expression complexity for a named sub-diagram to be
    /// extracted into its own file when folding is active.
    ///
    /// C++ global: `gFoldComplexity` (default 2). CLI: `-fc N` / `--fold-complexity N`.
    pub fold_complexity: usize,

    /// How the files are named: by box identity, as C++ does (the default),
    /// or by definition name, stable from one run to the next.
    pub file_naming: FileNaming,
}

impl Default for DrawConfig {
    fn default() -> Self {
        Self {
            shadow_blur: false,
            scaled_svg: false,
            draw_route_frame: false,
            max_name_size: 40,
            fold_threshold: 25,
            fold_complexity: 2,
            file_naming: FileNaming::BoxIdentity,
        }
    }
}

// ─── Public entry point ───────────────────────────────────────────────────────

/// One SVG file of a drawing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagram {
    /// What the diagram is: the root's title, or the definition name of a
    /// folded sub-diagram.
    pub name: String,
    /// Its file name, relative to the drawing's directory.
    pub file: String,
    /// The SVG document.
    pub svg: Vec<u8>,
}

/// Generate the SVG block diagram(s) of `root` into memory, root first, then
/// the folded sub-diagrams in the order they were scheduled.
///
/// `name` names the root's file (under `config.file_naming`) and `title` is
/// written above it; [`draw_schema`] and [`draw_schema_to_memory`] pass the
/// same string for both. A caller drawing an expression rather than a
/// definition gives the expression as the title and a stable name such as
/// `process` for the file.
///
/// # Errors
/// Returns [`DrawError`] if rendering fails.
///
/// C++ reference: `drawschema.cpp:149` — `drawSchema` / `writeSchemaFile`.
pub fn draw_diagrams(
    arena: &tlib::TreeArena,
    root: boxes::BoxId,
    name: &str,
    title: &str,
    config: &DrawConfig,
    def_names: &std::collections::HashMap<boxes::BoxId, String>,
) -> Result<Vec<Diagram>, DrawError> {
    use std::collections::{HashSet, VecDeque};
    use std::io::Cursor;

    use device::SvgDevice;
    use schema::TraitCollector;
    use translate::{DrawMemo, FileNames, FoldState, generate_folded_inside, make_top_schema};

    let mut memo = DrawMemo::new(arena, root);
    let folding = config.fold_threshold > 0 && memo.complexity(arena, root) > config.fold_threshold;

    // Queue: (box_id, diagram_name, back_link_file)
    let mut pending: VecDeque<(boxes::BoxId, String, String)> = VecDeque::new();
    let mut drawn: HashSet<boxes::BoxId> = HashSet::new();
    let mut files = FileNames::new(config.file_naming);
    let mut diagrams = Vec::new();

    pending.push_back((root, name.to_owned(), String::new()));
    drawn.insert(root);

    while let Some((box_id, diagram_name, back_link)) = pending.pop_front() {
        let file_name = files.assign(box_id, &diagram_name);
        let shown = if diagrams.is_empty() {
            title.to_owned()
        } else {
            diagram_name
        };

        let mut state = FoldState {
            def_names,
            pending: &mut pending,
            drawn: &mut drawn,
            current_file: file_name.clone(),
            folding,
            fold_complexity: config.fold_complexity,
            files: &mut files,
            memo: &mut memo,
        };

        let inner = match state.memo.take_prepared(box_id) {
            Some(prepared) => prepared,
            None => generate_folded_inside(arena, box_id, config, &mut state),
        };
        let mut top = make_top_schema(inner, &shown, &back_link);
        top.place(0.0, 0.0, Orientation::LeftRight);

        let mut buf = Cursor::new(Vec::new());
        let mut dev = SvgDevice::new(&mut buf, top.width(), top.height(), config)?;
        top.draw(&mut dev)?;
        let mut collector = TraitCollector::new();
        top.collect_traits(&mut collector);
        collector.draw(&mut dev)?;
        dev.finish().map(|_| ())?;
        diagrams.push(Diagram {
            name: shown,
            file: file_name,
            svg: buf.into_inner(),
        });
    }

    Ok(diagrams)
}

/// Generate SVG block-diagram file(s) into memory and return them as a
/// `(filename, bytes)` vector.
///
/// This is the filesystem-free counterpart of [`draw_schema`].  Every SVG is
/// rendered into a `Vec<u8>` buffer; nothing is written to disk.  Use this
/// variant when the host has no writable filesystem, such as when the
/// compiler runs as a `wasm32-unknown-unknown` module.
///
/// `process.svg` is always the first element of the returned vector when the
/// root diagram is named `"process"`, matching the hierarchy entry-point
/// convention expected by the `faustwasm` SVG map.
///
/// # Errors
/// Returns [`DrawError`] if rendering fails.
pub fn draw_schema_to_memory(
    arena: &tlib::TreeArena,
    root: boxes::BoxId,
    name: &str,
    config: &DrawConfig,
    def_names: &std::collections::HashMap<boxes::BoxId, String>,
) -> Result<Vec<(String, Vec<u8>)>, DrawError> {
    Ok(draw_diagrams(arena, root, name, name, config, def_names)?
        .into_iter()
        .map(|diagram| (diagram.file, diagram.svg))
        .collect())
}

/// Generate SVG block-diagram file(s) from a box expression.
///
/// Writes `process.svg` (and optionally sub-diagram files) into `out_dir`.
///
/// When `config.fold_threshold > 0` and the root diagram complexity exceeds
/// the threshold, named sub-diagrams are extracted into separate SVG files
/// with clickable hyperlinks — mirroring the C++ `-f N` folding behaviour.
///
/// `def_names` maps evaluated `BoxId` values to their Faust definition names,
/// as populated by the evaluator (`setDefNameProperty` equivalent).
///
/// # Errors
/// Returns [`DrawError::Io`] if any output file cannot be written.
///
/// C++ reference: `drawschema.cpp:149` — `drawSchema` / `writeSchemaFile`.
pub fn draw_schema(
    arena: &tlib::TreeArena,
    root: boxes::BoxId,
    name: &str,
    out_dir: &std::path::Path,
    config: &DrawConfig,
    def_names: &std::collections::HashMap<boxes::BoxId, String>,
) -> Result<(), DrawError> {
    for diagram in draw_diagrams(arena, root, name, name, config, def_names)? {
        std::fs::write(out_dir.join(&diagram.file), &diagram.svg).map_err(DrawError::Io)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use boxes::{BoxBuilder, BoxId};
    use tlib::TreeArena;

    use super::*;

    /// `process = (voice, voice'), Voice`, three distinct sub-diagrams sharing
    /// one name up to case, built after `padding` unrelated nodes so that
    /// every box index moves with it.
    fn program(padding: i32) -> (TreeArena, BoxId, HashMap<BoxId, String>) {
        let mut arena = TreeArena::new();
        let mut b = BoxBuilder::new(&mut arena);
        for k in 0..padding {
            let _ = b.int(1000 + k);
        }
        let (w1, w2, add) = (b.wire(), b.wire(), b.add());
        let pair = b.par(w1, w2);
        let voice = b.seq(pair, add);
        let one = b.int(1);
        let wire = b.wire();
        let pair1 = b.par(wire, one);
        let add1 = b.add();
        let voice2 = b.seq(pair1, add1);
        let delay = b.delay1();
        let voice3 = b.seq(voice, delay);
        let left = b.par(voice, voice2);
        let root = b.par(left, voice3);
        let def_names = HashMap::from([
            (voice, "voice".to_owned()),
            (voice2, "voice".to_owned()),
            (voice3, "Voice".to_owned()),
        ]);
        (arena, root, def_names)
    }

    fn draw(padding: i32) -> Vec<Diagram> {
        let (arena, root, def_names) = program(padding);
        let config = DrawConfig {
            fold_threshold: 1,
            fold_complexity: 1,
            file_naming: FileNaming::DefinitionName,
            ..DrawConfig::default()
        };
        draw_diagrams(&arena, root, "process", "the title", &config, &def_names).unwrap()
    }

    #[test]
    fn definition_names_disambiguate_in_drawing_order_without_case() {
        let diagrams = draw(0);
        let files: Vec<&str> = diagrams.iter().map(|d| d.file.as_str()).collect();
        // `+` is in both `voice` diagrams: shared, it is folded once, under
        // its expression, while `voice` is prepared, hence before `voice-2`
        assert_eq!(
            files,
            [
                "process.svg",
                "voice.svg",
                "diagram.svg",
                "voice-2.svg",
                "Voice-3.svg"
            ]
        );
        let names: Vec<&str> = diagrams.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["the title", "voice", "+", "voice", "Voice"]);
        // the links of the root name the files that were written
        let root = String::from_utf8(diagrams[0].svg.clone()).unwrap();
        for file in ["voice.svg", "voice-2.svg", "Voice-3.svg"] {
            assert!(root.contains(file), "process.svg does not link {file}");
        }
        assert!(root.contains("the title"));
    }

    fn svg(diagram: &Diagram) -> String {
        String::from_utf8(diagram.svg.clone()).unwrap()
    }

    fn folding(file_naming: FileNaming) -> DrawConfig {
        DrawConfig {
            fold_threshold: 1,
            fold_complexity: 2,
            file_naming,
            ..DrawConfig::default()
        }
    }

    #[test]
    fn a_shared_node_without_a_name_is_folded_once_and_linked_at_each_use() {
        // `s = 1, 2 : +` twice, named by nothing
        let mut arena = TreeArena::new();
        let mut b = BoxBuilder::new(&mut arena);
        let (one, two) = (b.int(1), b.int(2));
        let pair = b.par(one, two);
        let add = b.add();
        let s = b.seq(pair, add);
        let root = b.par(s, s);
        let none = HashMap::new();
        let config = folding(FileNaming::DefinitionName);
        let diagrams = draw_diagrams(&arena, root, "process", "process", &config, &none).unwrap();
        let files: Vec<&str> = diagrams.iter().map(|d| d.file.as_str()).collect();
        assert_eq!(files, ["process.svg", "diagram.svg"]);
        assert_eq!(diagrams[1].name, "1,2 : +");
        let top = svg(&diagrams[0]);
        assert!(top.contains("\"diagram.svg\""), "{top}");
        // folded, not expanded: the constants are drawn in diagram.svg only
        assert!(!top.contains(">1<") && !top.contains(">2<"), "{top}");
        assert!(svg(&diagrams[1]).contains(">2<"));

        // the C++ scheme names it `diagram-<hex>`, not after the expression
        let identity = folding(FileNaming::BoxIdentity);
        let diagrams = draw_diagrams(&arena, root, "process", "process", &identity, &none).unwrap();
        assert!(
            diagrams[1].file.starts_with("diagram-"),
            "{}",
            diagrams[1].file
        );
    }

    #[test]
    fn a_named_diagram_used_twice_is_drawn_once() {
        // C++ links every use of a scheduled diagram; Rust used to expand the
        // second one in place
        let mut arena = TreeArena::new();
        let mut b = BoxBuilder::new(&mut arena);
        let (one, two) = (b.int(1), b.int(2));
        let pair = b.par(one, two);
        let add = b.add();
        let s = b.seq(pair, add);
        let delay = b.delay1();
        let late = b.seq(s, delay);
        let root = b.par(s, late);
        let names = HashMap::from([(s, "voice".to_owned())]);
        let config = folding(FileNaming::DefinitionName);
        let diagrams = draw_diagrams(&arena, root, "process", "process", &config, &names).unwrap();
        let files: Vec<&str> = diagrams.iter().map(|d| d.file.as_str()).collect();
        assert_eq!(files, ["process.svg", "voice.svg"]);
        assert!(!svg(&diagrams[0]).contains(">1<"));
    }

    #[test]
    fn a_fibonacci_shaped_dag_draws_in_linear_size() {
        // f(k) = f(k-1), f(k-2) : +, whose expanded tree has about 1.6^k
        // boxes: k = 60 would be some 10^12. Constants at the bottom: with
        // `f(1) = _` the program itself has fib(k) inputs, and so many wires
        // are drawn whatever the folding.
        let mut arena = TreeArena::new();
        let mut b = BoxBuilder::new(&mut arena);
        let mut levels = vec![b.int(1), b.int(2)];
        for i in 2..=60 {
            let pair = b.par(levels[i - 1], levels[i - 2]);
            let add = b.add();
            levels.push(b.seq(pair, add));
        }
        let none = HashMap::new();
        let config = folding(FileNaming::DefinitionName);
        let diagrams =
            draw_diagrams(&arena, levels[60], "process", "process", &config, &none).unwrap();
        let bytes: usize = diagrams.iter().map(|d| d.svg.len()).sum();
        assert!(diagrams.len() < 70, "{} files", diagrams.len());
        assert!(bytes < 1_000_000, "{bytes} bytes");
        // every file of the drawing is linked from another one
        for d in &diagrams[1..] {
            let quoted = format!("\"{}\"", d.file);
            assert!(
                diagrams
                    .iter()
                    .any(|o| o.file != d.file && svg(o).contains(&quoted)),
                "{} is linked from nowhere",
                d.file
            );
        }
    }

    #[test]
    fn definition_names_do_not_move_with_box_indices() {
        // the same program built after other nodes: every index differs, the
        // files and their contents do not
        assert_eq!(draw(0), draw(37));
    }

    #[test]
    fn box_identity_names_keep_the_cpp_scheme() {
        let (arena, root, def_names) = program(0);
        let config = DrawConfig {
            fold_threshold: 1,
            fold_complexity: 1,
            ..DrawConfig::default()
        };
        let files = draw_schema_to_memory(&arena, root, "process", &config, &def_names).unwrap();
        assert_eq!(files[0].0, "process.svg");
        assert!(files[1].0.starts_with("voice-") && files[1].0.len() == "voice-".len() + 12);
    }
}
