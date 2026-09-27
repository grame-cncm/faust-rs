# Intégrer faust-rs : les couches d'API, et laquelle lier

faust-rs est écrit en Rust de bout en bout. Son API C est du code Rust exporté
avec l'édition de liens C : elle reproduit l'API C de libfaust, si bien qu'un
hôte C ou C++ peut lier `libfaust-rs` là où il liait libfaust. Un hôte atteint
le compilateur par l'une de quatre couches, et la bonne dépend du langage dans
lequel l'hôte, ou le binding, est écrit.

La description des couches et du compromis vient de shakfu, auteur de
`py-faust-rs`, dans
[l'issue #17](https://github.com/grame-cncm/faust-rs/issues/17#issuecomment-5856868752).

## Les quatre couches

```text
4  bindings vers d'autres langages  py-faust-rs (PyO3), ...       hors du workspace
3  API Rust                         faust                         Rust sûr, sans pointeur brut
2  API C                            interp-ffi, cranelift-ffi,    extern "C", pointeurs bruts
                                    box-ffi, signal-ffi,
                                    libfaust-ffi
                                    -> libfaust-rs (faust-ffi)
                                    wasm-ffi (faustwasm)
1  compilateur et backends          parser ... codegen, compiler  Rust pur
```

1. **Compilateur et backends.** La chaîne de `parser` à `codegen`, et la crate
   `compiler` qui la pilote (et porte la ligne de commande `faust-rs`). Rust
   pur, sans promesse de stabilité : ces crates changent selon les besoins du
   portage.
2. **API C.** Des fonctions Rust à signature C (`extern "C"`, pointeurs
   bruts, tables de callbacks `UIGlue` et `MetaGlue`) : les API de factory et
   d'instance de l'interpréteur et de Cranelift, les API Box et Signal, et les
   fonctions indépendantes du backend `expandDSP*` / `generateAuxFiles*` /
   `generateSHA1`. `faust-ffi` les réunit en une bibliothèque, `libfaust-rs`,
   avec ses en-têtes C et C++. `wasm-ffi` en est l'équivalent pour
   `faustwasm` : une ABI WASM brute autour du même compilateur.
3. **API Rust.** La crate `faust` : une `Factory` est un programme compilé, un
   `Dsp` une instance, les contrôles sont désignés par leur chemin
   `/groupe/label`. Elle appelle la couche 2 directement, comme des fonctions
   Rust, sans bibliothèque partagée entre les deux, si bien qu'une `Factory` a
   exactement le cycle de vie de l'API C (programmes partagés par clé SHA,
   comptés par références). Tout l'`unsafe` du chemin est dans cette crate ;
   son hôte n'en écrit pas.
4. **Bindings vers d'autres langages.** Par exemple `py-faust-rs`, qui expose
   la couche 3 à Python avec PyO3.

Les couches 2 et 3 sont les deux contrats de faust-rs. La couche 1 peut
changer sans préavis.

`cargo run -p xtask -- ffi-boundary-check` fait respecter le sens des
dépendances entre elles, sous les noms du workspace : la couche 1 est le
*cœur* (core), les crates de la couche 2 sont les *adaptateurs* (adapters), et
`faust-ffi`, `wasm-ffi` et `faust` sont les crates de *distribution*, ce qu'un
hôte lie. Aucune crate ne dépend d'une couche à sa droite, et seuls les
adaptateurs, les crates de distribution qui en ont besoin et le pont
d'exécution `foreign-call` peuvent autoriser `unsafe`.

## La couche 3 dans le code

L'API Rust est la crate `faust`, `crates/faust`. Sa surface publique est ce
que `src/lib.rs` définit et réexporte ; le reste est privé.

| Fichier | Contient |
| --- | --- |
| `src/lib.rs` | la documentation de la crate (modèle, précision, cycle de vie, lacune connue) ; `Backend`, `Precision`, `CompileOptions`, `Error`, `ErrorKind`, `version()` ; les réexports |
| `src/factory.rs` | `Factory` : `from_file`, `from_source`, `instantiate`, `json`, `name`, `backend`, `precision` |
| `src/dsp.rs` | `Dsp` : `compute_f32`, `compute_f64`, `controls`, `control`, `get`, `set`, `metadata`, les initialisations (`init`, `instance_init`, `reset_controls`, `clear`), les arités et la fréquence d'échantillonnage ; pourquoi il est `Send` et `Sync` |
| `src/controls.rs` | `Control`, `ControlKind` ; en privé, le parcours `UIGlue` qui trouve les contrôles et construit leurs chemins `MapUI`, et le collecteur `MetaGlue` de `Dsp::metadata` |
| `src/backend.rs` | privé : `RawFactory` et `RawInstance`, le seul endroit qui appelle les points d'entrée C de `interp-ffi` et `cranelift-ffi`, et donc l'`unsafe` de la crate |
| `tests/api.rs`, `tests/ddsp.rs`, `tests/allocation.rs` | le contrat sur les deux backends : cycle de vie, contrôles, précision, threads ; des programmes DDSP à travers l'API ; aucune allocation dans `compute` |

Sa documentation est celle de rustdoc : `cargo doc -p faust --open` la
produit, en commençant par la page de la crate. Elle se construit sans
avertissement, et la page de la crate décrit le modèle, mais tous les éléments
publics n'ont pas encore leur commentaire : les accesseurs simples
(`Dsp::backend`, `num_inputs`, `sample_rate`, `control`, ...), les champs de
`Control`, `CompileOptions` et `Error`, et les variantes de `ControlKind` et
`Precision` n'en ont pas, et les fonctions qui renvoient un `Result` n'ont pas
de section `# Errors`. Les lints listent ce qui manque :

```bash
cargo clippy -p faust --lib -- -W missing_docs -W clippy::missing_errors_doc
```

## Quelle couche lier

| L'hôte ou le binding est écrit en | Lier | Par |
| --- | --- | --- |
| C ou C++ | la couche 2 | `libfaust-rs` et ses en-têtes |
| Python avec Cython, cffi ou ctypes, ou tout langage doté d'une FFI C | la couche 2 | `libfaust-rs`, comme `cyfaust` lie la libfaust C++ |
| JavaScript, avec `faustwasm` | la couche 2 | `wasm-ffi` |
| Rust | la couche 3 | la crate `faust` |
| Rust, pour exposer faust-rs à un autre langage (PyO3, napi-rs, ...) | la couche 3 | la crate `faust` |

Un binding écrit en Rust a les deux à sa disposition, et c'est la couche 3
qu'il faut prendre. Lier la couche 2 depuis Rust coûterait :

- **du code `unsafe` dans le binding.** Les pointeurs bruts de factory et
  d'instance, leurs durées de vie et les tables de callbacks sont traités une
  fois, dans `faust`.
- **de refaire le parcours de l'interface utilisateur.** Lister les contrôles
  d'un programme demande de répondre aux callbacks `UIGlue` et de construire
  les chemins à la `MapUI` à partir des labels des groupes ; `faust` le fait
  (`Dsp::controls`, `Dsp::set`, `Dsp::get`).
- **des échantillons `f32` pour l'interpréteur.** Comme en C++, le point
  d'entrée C `computeCInterpreterDSPInstance` échange des `FAUSTFLOAT**`, des
  `float**` dans `libfaust-rs` : un programme interprété compilé en `-double`
  voit son entrée et sa sortie arrondies en `f32`. Le chemin `f64`,
  `interp_ffi::instance::compute_f64`, est une fonction Rust, hors de l'API C ;
  `faust` s'en sert, si bien que `Dsp::compute_f64` est exact sur les deux
  backends. (Le point d'entrée C de Cranelift calcule à la précision compilée
  derrière la même signature `float**`.)

Un binding qui n'est pas écrit en Rust n'a pas de couche 3 à atteindre : la
couche 2 est la sienne, et c'est le contrat que `libfaust-rs` tient avec les
hôtes C et C++.

## Un client Rust

La crate n'est pas publiée sur crates.io : un hôte en dépend par un chemin, ou
par une dépendance `git` sur le dépôt.

```toml
[dependencies]
faust = { path = "../faust-rs/crates/faust" }
```

Un programme complet : il compile un lisseur à un pôle pour le JIT Cranelift
en double précision, liste ses contrôles, en règle un par son chemin, calcule
un bloc, puis passe l'instance à un autre thread, après que l'hôte a lâché sa
factory.

```rust
use faust::{Backend, CompileOptions, ErrorKind, Factory, Precision};

/// Un lisseur à un pôle : son pôle est un curseur.
const SOURCE: &str = r#"
process = _ * (1 - p) : + ~ *(p)
with { p = hslider("pole [style:knob]", 0.9, 0, 0.999, 0.001); };
"#;

fn main() -> Result<(), faust::Error> {
    // Compiler une fois, pour le JIT Cranelift, en double précision (`-double`).
    let options = CompileOptions {
        backend: Backend::Cranelift,
        precision: Precision::F64,
        ..CompileOptions::default()
    };
    let factory = Factory::from_source("smoother", SOURCE, &options)?;
    let mut dsp = factory.instantiate(48_000)?;

    // Les contrôles, désignés par leur chemin `MapUI`/OSC.
    for control in dsp.controls() {
        println!(
            "{} {:?} in [{}, {}], now {}",
            control.path, control.kind, control.min, control.max, dsp.get(&control.path)?
        );
    }
    let pole = dsp.control("/smoother/pole").expect("declared by the program");
    let value = pole.clamp(0.99); // `set` écrit la valeur telle quelle
    dsp.set("/smoother/pole", value)?;

    // Un chemin mal orthographié est une erreur, jamais ignoré en silence.
    let error = dsp.set("/smoother/pol", 0.5).unwrap_err();
    assert_eq!(error.kind, ErrorKind::UnknownControl);

    // Un bloc de la réponse impulsionnelle, en `f64` puisque le programme est `-double`.
    let mut input = vec![0.0_f64; 64];
    input[0] = 1.0;
    let mut output = vec![0.0_f64; 64];
    dsp.compute_f64(&[&input], &mut [&mut output])?;
    println!("impulse response: {:?}", &output[..3]);

    // Un `Dsp` est `Send` et garde son programme en vie : l'hôte peut lâcher
    // sa factory et passer l'instance à un thread audio. Des buffers de
    // l'autre largeur sont convertis.
    drop(factory);
    let audio = std::thread::spawn(move || -> Result<f32, faust::Error> {
        let silence = [0.0_f32; 64];
        let mut out = [0.0_f32; 64];
        dsp.compute_f32(&[&silence], &mut [&mut out])?;
        Ok(out[0])
    });
    let next = audio.join().expect("the audio thread")?;
    println!("next block starts at {next}");
    Ok(())
}
```

Il affiche :

```text
/smoother/pole HorizontalSlider in [0, 0.999], now 0.9
impulse response: [0.010000000000000009, 0.00990000000000001, 0.00980100000000001]
next block starts at 0.005255965
```

`Backend::Interp` à la place de `Backend::Cranelift` fait tourner le même
programme sur l'interpréteur, avec les mêmes résultats ; `Factory::from_file`
compile un fichier `.dsp`, et `CompileOptions::import_dirs` et `args` portent
`-I` et les autres options du compilateur.

## Voir aussi

- [README : utiliser `libfaust-rs` depuis C et C++](../README.md#use-libfaust-rs-from-c-and-c),
  l'API C et l'API Rust, avec des exemples.
- La documentation de la crate `faust` : `cargo doc -p faust --open`.
- [`crates/faust-ffi`](../crates/faust-ffi/README.md), la construction de
  `libfaust-rs` ; [`crates/wasm-ffi`](../crates/wasm-ffi/README.md), le module
  compilateur de `faustwasm`.
