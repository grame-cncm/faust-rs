# Plan de structure et de lisibilité de faust-rs

**Date :** 2026-08-18
**Statut :** Phase 0 — audit et plan seulement. Aucun code n'a été modifié.
**Périmètre :** l'ensemble du workspace (31 crates, 272 411 lignes de Rust).
**Jumeau anglais :** [`faust-rs-structure-readability-plan-2026-08-18-en.md`](faust-rs-structure-readability-plan-2026-08-18-en.md) (l'anglais fait foi).
**Documents compagnons :** [`delay-rs-simplification-experiment-2026-06-21-fr.md`](delay-rs-simplification-experiment-2026-06-21-fr.md),
[`signal-prepare-simplification-experiment-2026-06-22-en.md`](signal-prepare-simplification-experiment-2026-06-22-en.md),
[`faust-rust-porting-plan-en.md`](faust-rust-porting-plan-en.md).

**But :** rendre le compilateur lisible, documentable et modifiable par morceaux
indépendants, en émettant un FIR byte-identique. C'est un travail
d'organisation — frontières de fichiers, nommage, en-têtes de module — pas une
réécriture, ni une optimisation, ni une correction de bugs.

---

## 0. Comment rejouer chaque chiffre de ce document

Toutes les métriques viennent d'un script versionné avec ce plan :
`porting/scripts/structure-metrics.py`. C'est un analyseur d'accolades qui neutralise
commentaires et littéraux de chaînes avant de compter ; il est délibérément
syntaxique et sous-compte plutôt que de deviner.

```bash
python3 porting/scripts/structure-metrics.py > /tmp/metrics.json
```

Aucun chiffre de ce document n'est une estimation.

---

## 1. La chaîne de compilation telle qu'elle est

### 1.1 Crates d'étape

| Crate | Rôle | Point d'entrée public | Lignes (prod / test) |
|---|---|---|---|
| `parser` | source Faust → AST de boxes (`lrpar`/`lrlex`) | `parse_program`, `parse_file*` | 6 011 / 1 577 |
| `boxes` | construction et filtrage de boxes sur `tlib::TreeArena` | `BoxBuilder`, `match_box` | 3 804 / 773 |
| `eval` | évaluateur de boxes (phase 4), filtrage de motifs fusionné | `eval_entrypoint*`, `eval_process*` | 8 670 / 2 177 |
| `propagate` | propagation boxes → signaux, AD directe/inverse | `propagate_typed*` | 10 878 / 2 571 |
| `signals` | construction et filtrage de signaux | `SigBuilder`, `match_sig` | 3 250 / 699 |
| `normalize` | normalisation et simplification algébrique | `normalize_signal` | 4 546 / 0 (inline) |
| `sigtype` | système de types des signaux, treillis à intervalles | `infer` | 4 150 / 0 (inline) |
| `transform` | abaissement intermédiaire : `signal_prepare` → `signal_fir` → vector | `compile_signals_to_fir_fastlane_*` (5 variantes) | 47 069 / 15 333 |
| `fir` | dépôt FIR, filtrage, vérificateur, inliner | `FirBuilder`, `match_fir`, `check_fir` | 10 493 / 4 071 |
| `codegen` | émission par backend depuis le FIR (17 répertoires) | `emit_*` par backend | 45 933 / 8 930 |
| `compiler` | façade de haut niveau : API bibliothèque + CLI `faust-rs` | `compile_source_to_*`, `main.rs` | 16 011 / **21 449** |

Crates de support : `tlib`, `interval`, `ui`, `diagnostics`, `draw`,
`foreign-call`, `xtask`, plus huit crates `*-ffi`.

### 1.2 Forme des dépendances

Le graphe généré (`docs/code-graphs/internal-crate-deps.dot`, 102 arêtes) montre
un empilement propre : `compiler` dépend de 14 crates, `codegen` seulement de
`fir` et `foreign-call`, les adaptateurs FFI sont strictement en aval. Aucun
cycle. **L'architecture au niveau des crates n'est pas le problème** — cet audit
n'a trouvé aucun défaut de direction de dépendance. Les problèmes sont *dans* les
crates et à leurs points d'entrée.

### 1.3 Écarts avec `AGENTS.md` §2

Trois intégrations y sont affirmées, vérifiées une par une :

- **`patternmatcher` fusionné dans `eval`** — confirmé
  (`crates/eval/src/pattern_matcher.rs`, utilisé depuis `apply.rs`).
- **`parallelize` intégré à `transform`** — confirmé, sous la forme de
  `crates/transform/src/schedule/`.
- **nœuds mathématiques `extended` intégrés à `signals`** — **partiellement
  trompeur.** Les constructeurs sont bien dans `signals` (`SigBuilder::acos`,
  `::atan2`…), mais le mot « extended » ne survit que dans
  `crates/boxes/src/print.rs` et dans les tables d'opcodes de l'interpréteur.
  L'affirmation est vraie sur le fond et périmée dans son vocabulaire : un
  lecteur qui cherche `extended` atterrit dans la mauvaise crate.

À enregistrer aussi : **trois membres du workspace sont déclarés placeholders** —
`graph` (21 lignes), `doc` (22), `algebra` (29). Ce sont de vrais membres Cargo
sans implémentation ; toute vue structurelle du workspace le surestime de trois.

---

## 2. Métriques mesurées

### 2.1 Volume

272 411 lignes de Rust : **212 571 de production, 59 840 dans des fichiers
`tests/` ou `tests.rs`**, plus ~28 800 en modules `#[cfg(test)]` inline —
**32 % du Rust est du test**, ratio normal voire bas pour un compilateur, et ce
n'est pas une cible de restructuration.

Deux crates portent 44 % du code de production : `transform` (47 069) et
`codegen` (45 933). `compiler` vient ensuite avec 16 011 — mais il porte
**21 449 lignes de test, plus de test que de production**, et 652 des ~2 500
fonctions `#[test]` du workspace. Un quart de la suite est au sommet de la
chaîne, là où un échec désigne le compilateur entier plutôt qu'une étape. C'est
un problème d'altitude à consigner, pas une phase de ce plan.

### 2.2 Fichiers de production > 1 500 lignes : 31

Les plus gros : `fir/src/checker.rs` (3 336), `codegen/…/rust/mod.rs` (3 278),
`codegen/…/cpp/mod.rs` (3 186), `parser/src/lib.rs` (3 150),
`codegen/…/wasm/mod.rs` (3 126), `fir/src/inliner.rs` (2 905),
`box-ffi/src/lib.rs` (2 855), `cranelift-ffi/src/factory.rs` (2 665),
`sigtype/src/rules.rs` (2 610), `codegen/…/c/mod.rs` (2 493),
`eval/src/lib.rs` (2 419), `transform/…/vector/lower/signal.rs` (2 339),
`compiler/src/lib.rs` (2 304).

Le seuil `MAX_PRODUCTION_LINES` de `structure-check` vaut 2 400 et ne s'applique
qu'à `transform` et `compiler` : **onze fichiers au-dessus vivent dans des crates
que la barrière ne regarde pas.**

### 2.3 Fonctions > 150 lignes : 116

1 640 `try_execute_block_io_inner` (`codegen/…/interp/executor.rs:527`),
1 449 `compile_instr` (`codegen/…/interp/fbc_to_cpp.rs:490`),
822 `artifacts_to_json` (`wasm-ffi`), 773 `propagate_inner`,
767 `build_module` (`transform`), 727 `run_source_mode` (`compiler`),
676 `match_fir` (`fir`), 643 `verify_vector_plan` (`transform`).

### 2.4 Blocs `impl` > 500 lignes : 24

Le plus gros : `fir/src/checker.rs:582` (2 649 lignes), puis
`codegen/…/interp/compiler.rs:218` (1 891),
`transform/…/vector/lower/signal.rs:461` (1 740),
`codegen/…/interp/executor.rs:443` (1 725).

### 2.5 Profondeur des modules

Profondeur maximale sous `src/` : **3**, atteinte seulement par `codegen` et
`transform`. L'arborescence est plate ; **ce n'est pas un problème ici.**

### 2.6 Couverture documentaire

- **Modules sans en-tête `//!` : 7** sur ~700. Le code est bien documenté au
  niveau module ; **ce n'est pas un levier.**
- **Items publics sans rustdoc : 251**, concentrés dans `codegen` (67),
  `transform` (57), `interval` (20), `interp-ffi` (20), `draw` (19).
  Seul `transform` a un plancher `missing_docs`.

### 2.7 Retours en tuple anonyme ≥ 3 champs : 13

Dix des treize sont des fixtures de test. **Pas un levier non plus** — le
soupçon formulé dans la commande n'est pas confirmé.

### 2.8 Le constat qui domine : l'accrétion de paramètres

**240 fonctions de production prennent plus de 6 paramètres.** Extrêmes : 22
(`interp/factory.rs:86`), 16 (`wasm/mod.rs:789`), 16 (`cranelift/jit_data.rs:255`),
15 (`compiler/src/signal_lowering.rs:605`), 11 (`compile_fastlane_inner`).

La même pression affleure à chaque frontière d'étape sous forme de **points
d'entrée télescopiques** — une fonction par combinaison d'arguments optionnels,
la combinaison étant épelée dans le nom :

| Étape | Variantes |
|---|---|
| `parser` | 6 (`parse_program`, `…_with_metadata`, `…_with_precision_and_metadata`, `…_with_imports_and_metadata`, …) |
| `parser` | 5 (famille `parse_file_with_imports*`) |
| `eval` | 5 (`eval_entrypoint`, `…_with_source_context`, `…_with_stats`, …) |
| `eval` | 4 (famille `eval_process*`) |
| `transform` | 5 (`compile_signals_to_fir_fastlane_with_ui`, `…_and_shadow`, `…_clocked`, `…_clocked_with_timing`, `…_clocked_with_timing_and_origins`) |
| `propagate` | 3 (`propagate_typed`, `…_with_ui`, `…_with_ui_options`) |

Dans `transform`, les cinq délèguent à un unique `compile_fastlane_inner` à 11
paramètres positionnels, dont trois `Option<…>` passés à `None` par les variantes
courtes.

### 2.9 Duplication inter-backends dans `codegen` — plus faible qu'annoncé

Similarité deux à deux des `mod.rs` des sept émetteurs textuels (lignes
normalisées, `difflib`) : **21 % à 57 %**, la paire la plus proche étant c/cpp
(57 %). Sur 3 382 lignes significatives (≥ 40 caractères), 838 occurrences
(279 lignes distinctes) apparaissent dans au moins deux backends — et les plus
partagées sont du passe-partout : le type `CodegenError` avec ses impls
`new`/`Display`/`Error`, et `decode_module`, dupliqués dans les sept.

`c_family.rs` (1 291 lignes) factorise déjà la paire c/cpp (20 et 22 références) ;
`cmajor`, `codebox` et `rust` ne le référencent pas du tout.

**Cela contredit la prémisse selon laquelle `codegen` est l'endroit où
restructurer réduit le volume.** Ses 45 933 lignes de production sont dominées
par `interp` (12 848) et `cranelift` (4 620) — deux machines réellement
grosses — et non par des émetteurs jumeaux, qui sont à 21–57 % semblables et
contiennent 7 à 34 % de test inline. La duplication extractible se compte en
centaines de lignes, pas en milliers.

---

## 3. Diagnostic de lisibilité, par coût pour le lecteur

**D1 — Le contexte optionnel est encodé dans les noms et les positions, pas dans
les types.** 240 fonctions au-dessus de 6 paramètres, six familles télescopiques
dans cinq crates. Pour appeler une étape il faut savoir laquelle de cinq noms
quasi identiques porte l'argument voulu ; pour en lire une, il faut compter des
`None` positionnels. C'est le défaut au rayon d'action le plus large, parce
qu'il siège exactement là où un lecteur s'oriente. C'est aussi le moins cher à
corriger : le remède — une structure d'options par frontière d'étape — est de la
pure délégation.

**D2 — Deux fonctions de l'interpréteur sont plus longues que la plupart des
crates.** `try_execute_block_io_inner` (1 640 lignes) et `compile_instr` (1 449)
tiennent chacune l'intégralité d'un aiguillage. Aucune documentation de module ne
rend lisible une fonction de 1 640 lignes ; il faut la découper par famille
d'opcodes. Avec `executor.rs`, `compiler.rs` et `fbc_to_cpp.rs` (2 170 / 2 225 /
2 255), c'est la région la plus dense et la moins lisible du workspace.

**D3 — `fir/checker.rs` : un `impl` de 2 649 lignes dans un fichier de 3 336.**
C'est l'autorité de validité du FIR, consommée par tous les backends. Sa taille
est un risque de revue précisément parce que c'est lui qui décide si tout le
reste est correct.

**D4 — `structure-check` surveille 2 crates sur 31.** Son seuil de 2 400 lignes
ne s'applique qu'à `transform` et `compiler`. Onze fichiers de production
au-dessus vivent dans `fir`, `codegen`, `parser`, `sigtype`, `box-ffi` et
`cranelift-ffi`, où rien ne les mesure. La barrière ne ment pas, mais elle est
lue comme si elle couvrait le workspace.

**D5 — 251 items publics sans rustdoc**, et un seul plancher `missing_docs`.

**D6 — Trois crates placeholders** gonflent toute vue structurelle.

**Explicitement pas des problèmes** — mesurés puis écartés, donc aucune phase n'y
consacrera d'effort : la profondeur d'arborescence (max 3), les en-têtes de
module manquants (7 fichiers), les tuples anonymes (13, surtout des fixtures), la
direction des dépendances (propre, sans cycle) et le volume de tests (32 %).

---

## 4. Ce que « restructuré » voudra dire

1. **Un fichier nomme une étape**, énonçable en une phrase. Là où c'est
   impossible aujourd'hui, le fichier est découpé, pas annoté.
2. **Le contexte optionnel devient un type nommé.** Aucune frontière d'étape ne
   gagne de nouvelle variante `_with_x` ; les entrées optionnelles deviennent des
   champs d'une structure d'options documentée avec un `Default`.
3. **Chaque module garde son en-tête `//!`**, répondant à : ce qu'il fait, ce qui
   entre et sort, ce qu'il garantit, quelle source C++ il reflète
   (`master-dev-ocpp-od-fir-2-FIR19` / `8eebea429`).
4. **La séparation producteur/vérificateur est préservée et étendue, jamais
   affaiblie.**
5. **Lire une étape ne doit pas exiger de lire ses voisines.**
6. **Dispatch statique seulement** — enums et monomorphisation, aucun `dyn`
   introduit pour factoriser.

---

## 5. Phases

Chaque phase est indépendante et sécable : abandonner la phase N+1 laisse le
dépôt cohérent. Chacune procède en *déplacer d'abord, éditer ensuite*.

### P1 — Replier la famille de points d'entrée de `transform` (première phase recommandée)

**Cible :** les cinq `compile_signals_to_fir_fastlane_*`
(`crates/transform/src/signal_fir/mod.rs:508-663`) et leur
`compile_fastlane_inner` à 11 paramètres.
**Transformation :** un point d'entrée unique prenant `&SignalFirRequest`, une
structure documentée dont les champs sont les arguments positionnels d'aujourd'hui,
`clock_domains`/`timing_sink`/`signal_origins` en `Option`, avec un `Default`.
Migration des appelants internes (`compiler/src/signal_lowering.rs`, tests).
**Preuve de neutralité :** pure délégation, aucune logique touchée. Diff de FIR
doré sur le corpus d'impulsions + suite complète. Le diff de
`docs/code-graphs/public-api-baseline.txt` est *attendu ici* et constitue la trace
relisible du déplacement de frontière.
**Critères de passage :** toutes les barrières vertes ; le diff de baseline montre
exactement les variantes retirées et le nouvel entrant ; `signal_fir/mod.rs` perd
≥ 100 lignes. **Un seul commit.**

### P2 — Même traitement, une crate par commit : `parser`, `eval`, `propagate`

Quatre familles de plus (6 + 5 + 5 + 4 + 3 variantes), forme identique à P1,
seulement après que P1 a prouvé la méthode de bout en bout.

### P3 — Découper le backend interpréteur

**Cible :** `codegen/src/backends/interp/{executor,compiler,fbc_to_cpp}.rs`
(6 650 lignes, contenant les fonctions de 1 640 et 1 449 lignes).
**Transformation :** découpage par famille d'opcodes en modules frères, sur le
modèle de l'arborescence `vector/` que R3 a produite dans `transform`.
**Preuve de neutralité :** le FIR byte-identique ne suffit pas ici (l'interpréteur
*exécute*) ; il faut l'oracle d'impulsions et le contrôle de parité
`opt_level=0` vs `max` exigé par `AGENTS.md` §5.
**Critères :** aucun fichier de production > 1 500 lignes sous `interp/`, aucune
fonction > 400.

### P4 — Extraire le passe-partout dupliqué des backends

**Cible :** `CodegenError` (+ `new`/`Display`/`Error`) et `decode_module`,
dupliqués à l'identique dans les sept backends textuels.
**Preuve :** backend par backend, sortie émise byte-identique sur le corpus doré,
**établie avant** que l'abstraction n'atterrisse.
**Taille attendue :** quelques centaines de lignes retirées, pas des milliers.
À faire, mais pas à mettre en tête.

### P5 — Découper `fir/checker.rs`

L'`impl` de 2 649 lignes éclaté selon les familles de règles vérifiées, en
préservant l'indépendance du vérificateur. Placé après P3 car c'est l'artefact le
plus risqué : tout l'aval lui fait confiance.

### P6 — Étendre le plancher structurel

Faire porter le seuil de `structure-check` sur tout le workspace au lieu de deux
crates, et **abaisser** `MAX_PRODUCTION_LINES` à mesure que P3/P5 le rendent
atteignable. Ajouter un plancher `missing_docs` par crate nettoyée. **Cette phase
enregistre le résultat des autres** ; elle ne peut pas passer en premier.

### P7 — Trancher le sort des crates placeholders

`graph`, `doc`, `algebra` : implémenter, replier dans leurs consommateurs, ou
retirer du workspace. Une décision, pas un refactor — elle demande l'intention du
mainteneur.

---

## 6. Ordre recommandé, et où il contredit la commande

La commande proposait `transform` → `codegen` → `compiler` par taille, avec
`codegen` comme « seul endroit où la restructuration réduit du volume ». **Les
mesures contredisent cela sur deux points** :

- `signal_fir/vector/**` de `transform` est **déjà** restructuré — 13 sous-modules
  cohérents de 2 000 à 3 500 lignes, produit du travail R3. L'attaquer comme cible
  de masse referait un travail fait. Ses défauts restants sont un fichier
  surdimensionné (`lower/signal.rs`) et ses points d'entrée (P1).
- Les émetteurs jumeaux de `codegen` sont à 21–57 % semblables, pas
  quasi-identiques, et 7 à 34 % de chacun est du test inline. Le volume est dans
  `interp` et `cranelift`, gros parce que ce sont des machines, pas parce qu'ils
  sont dupliqués.

Ordre recommandé : **P1 → P2 → P3 → P4 → P5 → P6**, P7 étant à soulever dès que le
mainteneur veut trancher. La logique est « la plus petite instance prouvable
d'abord » : P1 est un fichier, une transformation mécanique, et il exerce toutes
les barrières y compris la baseline d'API publique ajoutée le 2026-08-18 — si la
méthode est fausse, c'est là que ça se voit au moindre coût.

---

## 7. Ce que ce plan ne fera pas

- **Toucher un abaissement numériquement sensible sans oracle.** Le WASM
  vectoriel n'a pas de référence C++ (`G5-W5`/`G5-W6`) ; toute phase qui
  l'atteindrait ne s'appuie que sur les tests du dépôt et doit le dire.
- **Remodeler du code qui reflète volontairement la structure C++** pour la revue
  de parité : cet alignement est un actif.
- **Fusionner des étapes qui se ressemblent** sans preuve qu'elles ont le même rôle.
- **Corriger les bugs rencontrés** : ils sont signalés, pas repliés dans un commit
  de restructuration.
- **Réduire la suite de tests.** À 32 % elle est proportionnée ; c'est elle qui
  rend ce chantier sûr.
- **Invoquer le FIR doré pour `xtask`, `cranelift-ffi` ou les crates `*-ffi`.**
  Ils n'émettent pas de FIR ; leur seule preuve de neutralité est la suite de
  tests, et aucune phase ne doit prétendre le contraire.
