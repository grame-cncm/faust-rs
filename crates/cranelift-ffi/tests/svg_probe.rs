//! `--svg DIR`: the block diagrams of the probed program, drawn by faust-rs's
//! `draw` into a directory the caller names, under names that do not change
//! from one run to the next.

mod common;
use common::{Fixtures, probe_in};

/// A library with a factory applied twice to different arguments: two
/// distinct diagrams named `voice`, which `--fold 1` puts in files of their
/// own.
const LIB: &str = r#"voice(n) = _ <: par(i, n, @(i + 1) : *(0.5)) :> _;
pair(x) = x, x * 0.25;
"#;

const DSP: &str = r#"v = library("voices.lib");
process = v.voice(3) , v.voice(5);
"#;

/// The rows of the CSV index, without its header, as `(file, name)`.
fn index(stdout: &str) -> Vec<(String, String)> {
    let mut lines = stdout.lines();
    assert_eq!(lines.next(), Some("file,name"), "{stdout}");
    lines
        .map(|line| {
            let (file, name) = line.split_once(',').expect("file,name");
            (file.to_owned(), name.to_owned())
        })
        .collect()
}

/// The files of `dir`, sorted, with their contents.
fn contents(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("read the svg dir")
        .map(|entry| {
            let path = entry.expect("an entry").path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read(&path).expect("read an svg"))
        })
        .collect();
    files.sort();
    files
}

#[test]
fn file_names_come_from_definitions_and_do_not_change_between_runs() {
    let fixtures = Fixtures::new("stable");
    fixtures.write("voices.lib", LIB);
    let dsp = fixtures.write("two.dsp", DSP);
    let run = |dir: &str| {
        let (ok, stdout, stderr) = probe_in(fixtures.dir(), &["--svg", dir, "--fold", "1", &dsp]);
        assert!(ok, "{stderr}");
        index(&stdout)
    };
    let first = run("a");
    let second = run("b");
    assert_eq!(first, second);
    let files: Vec<&str> = first.iter().map(|(file, _)| file.as_str()).collect();
    // the two `voice` diagrams, in drawing order; the sub-diagrams they
    // share, unnamed, are `diagram.svg`, `diagram-2.svg`, ...
    assert_eq!(files[0], "process.svg");
    let voices: Vec<&str> = first
        .iter()
        .filter(|(_, name)| name == "voice")
        .map(|(file, _)| file.as_str())
        .collect();
    assert_eq!(voices, ["voice.svg", "voice-2.svg"]);
    // no name carries a box index, `voice-000000c2.svg`
    let indexed = |file: &str| {
        file.strip_suffix(".svg")
            .and_then(|stem| stem.rsplit_once('-'))
            .is_some_and(|(_, tail)| tail.len() == 8 && tail.chars().all(|c| c.is_ascii_hexdigit()))
    };
    assert!(!files.iter().any(|f| indexed(f)), "{files:?}");
    // what the index lists is what was written, byte for byte the same twice
    let a = contents(&fixtures.dir().join("a"));
    let b = contents(&fixtures.dir().join("b"));
    assert_eq!(a, b);
    let mut listed: Vec<&str> = files.clone();
    listed.sort_unstable();
    let written: Vec<&str> = a.iter().map(|(file, _)| file.as_str()).collect();
    assert_eq!(written, listed);
    // and the root links the files by those names
    let root = String::from_utf8(
        a.iter()
            .find(|(f, _)| f == "process.svg")
            .unwrap()
            .1
            .clone(),
    )
    .unwrap();
    assert!(root.contains("\"voice.svg\"") && root.contains("\"voice-2.svg\""));
}

#[test]
fn svg_writes_only_into_its_directory() {
    let fixtures = Fixtures::new("only_dir");
    fixtures.write("voices.lib", LIB);
    let dsp = fixtures.write("two.dsp", DSP);
    let (ok, _, stderr) = probe_in(fixtures.dir(), &["--svg", "figs/two", &dsp]);
    assert!(ok, "{stderr}");
    let mut entries: Vec<String> = std::fs::read_dir(fixtures.dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    // no `two-svg/` next to the source: the directory asked for, created
    assert_eq!(entries, ["figs", "two.dsp", "voices.lib"]);
    assert!(
        fixtures
            .dir()
            .join("figs")
            .join("two")
            .join("process.svg")
            .exists()
    );
}

#[test]
fn an_expression_of_a_library_is_drawn_titled_with_its_text() {
    let fixtures = Fixtures::new("eval");
    let lib = fixtures.write("voices.lib", LIB);
    let (ok, stdout, stderr) = probe_in(
        fixtures.dir(),
        &["--svg", "out", "--eval", "voice(4), pair(2.0)", &lib],
    );
    assert!(ok, "{stderr}");
    let mut lines = stdout.lines();
    assert_eq!(lines.next(), Some("file,name"));
    // the expression holds a comma: the field is quoted
    assert_eq!(lines.next(), Some("process.svg,\"voice(4), pair(2.0)\""));
    let root = std::fs::read_to_string(fixtures.dir().join("out").join("process.svg")).unwrap();
    assert!(
        root.contains("voice(4), pair(2.0)"),
        "the title is the expression"
    );
    assert!(!root.contains("__faustprobe"), "the wrapper does not show");
}

#[test]
fn several_expressions_are_labelled_with_their_text() {
    let fixtures = Fixtures::new("evals");
    let lib = fixtures.write("voices.lib", LIB);
    let (ok, stdout, stderr) = probe_in(
        fixtures.dir(),
        &[
            "--svg",
            "out",
            "--fold",
            "1",
            "--eval",
            "voice(3)",
            "--eval",
            "pair(2.0)",
            &lib,
        ],
    );
    assert!(ok, "{stderr}");
    let names: Vec<String> = index(&stdout).into_iter().map(|(_, name)| name).collect();
    assert_eq!(names[0], "\"voice(3), pair(2.0)\"");
    assert!(!stdout.contains("__faustprobe"), "{stdout}");
}

#[test]
fn a_bad_expression_is_an_error_located_in_it_and_writes_nothing() {
    let fixtures = Fixtures::new("bad");
    let lib = fixtures.write("voices.lib", LIB);
    let (ok, stdout, stderr) = probe_in(
        fixtures.dir(),
        &["--svg", "out", "--eval", "voice(3) : nosuch", &lib],
    );
    assert!(!ok);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("nosuch"), "{stderr}");
    assert!(stderr.contains("<eval 0>"), "{stderr}");
    assert!(!fixtures.dir().join("out").exists());
}

#[test]
fn the_process_name_names_the_root() {
    let fixtures = Fixtures::new("pn");
    fixtures.write("voices.lib", LIB);
    let dsp = fixtures.write("two.dsp", &format!("{DSP}other = v.voice(2);\n"));
    let (ok, stdout, stderr) = probe_in(fixtures.dir(), &["--svg", "out", "-pn", "other", &dsp]);
    assert!(ok, "{stderr}");
    assert_eq!(
        index(&stdout)[0],
        ("other.svg".to_owned(), "other".to_owned())
    );
}

#[test]
fn simplified_diagrams_show_constant_sub_diagrams_as_values() {
    let fixtures = Fixtures::new("sd");
    // the evaluator folds `2 + 3` by itself; this one it leaves to `-sd`
    let dsp = fixtures.write("k.dsp", "process = _, (1 <: _, _ : +) : *;\n");
    let labels = |extra: &[&str], dir: &str| {
        let args = [&["--svg", dir], extra, &[dsp.as_str()]].concat();
        let (ok, _, stderr) = probe_in(fixtures.dir(), &args);
        assert!(ok, "{stderr}");
        std::fs::read_to_string(fixtures.dir().join(dir).join("process.svg")).unwrap()
    };
    let plain = labels(&[], "plain");
    let simplified = labels(&["-sd"], "simplified");
    assert!(plain.contains(">1<") && plain.contains(">+<"), "{plain}");
    assert!(simplified.contains(">2<"), "{simplified}");
    assert!(!simplified.contains(">+<"), "{simplified}");
}

#[test]
fn the_index_is_a_json_document_with_format_json() {
    let fixtures = Fixtures::new("json");
    fixtures.write("voices.lib", LIB);
    let dsp = fixtures.write("two.dsp", DSP);
    let (ok, stdout, stderr) = probe_in(
        fixtures.dir(),
        &["--svg", "out", "--fold", "1", "--format", "json", &dsp],
    );
    assert!(ok, "{stderr}");
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("one JSON document");
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["svg_dir"], "out");
    assert_eq!(document["diagrams"][1]["file"], "voice.svg");
    assert_eq!(document["diagrams"][1]["name"], "voice");
}

#[test]
fn render_flags_and_stray_drawing_options_are_refused() {
    let fixtures = Fixtures::new("refused");
    let dsp = fixtures.write("k.dsp", "process = *(hslider(\"g\", 0.5, 0, 1, 0.01));\n");
    for args in [
        &["--svg", "out", "--set", "g=1", dsp.as_str()][..],
        &["--svg", "out", "-n", "10", dsp.as_str()][..],
        &["--svg", "out", "--freqresp", "8", dsp.as_str()][..],
        &["--svg", "out", "--format", "ir", dsp.as_str()][..],
        // a drawing option without --svg would be ignored
        &["--fold", "0", dsp.as_str()][..],
        &["-sd", dsp.as_str()][..],
    ] {
        let (ok, _, stderr) = probe_in(fixtures.dir(), args);
        assert!(!ok, "{args:?} was accepted");
        assert!(!stderr.is_empty(), "{args:?}");
    }
    assert!(!fixtures.dir().join("out").exists());
}

#[test]
fn a_recursion_with_shared_subproblems_draws_in_linear_size() {
    // cheb(n) shares cheb(n-1) and cheb(n-2) with its neighbours: expanded,
    // cheb(30) is some 10^6 boxes and its drawing several GB (C++ before
    // #1290, faust-rs before the port); each distinct one is drawn once
    let fixtures = Fixtures::new("cheb");
    let dsp = fixtures.write(
        "cheb.dsp",
        "cheb(0,x) = 1;\ncheb(1,x) = x;\ncheb(n,x) = 2*x*cheb(n-1, x) - cheb(n-2, x);\n\
         process = cheb(30);\n",
    );
    let (ok, stdout, stderr) = probe_in(fixtures.dir(), &["--svg", "out", &dsp]);
    assert!(ok, "{stderr}");
    let written = contents(&fixtures.dir().join("out"));
    let bytes: usize = written.iter().map(|(_, svg)| svg.len()).sum();
    assert_eq!(written.len(), index(&stdout).len());
    assert!(written.len() < 100, "{} files", written.len());
    assert!(bytes < 2_000_000, "{bytes} bytes");
}
