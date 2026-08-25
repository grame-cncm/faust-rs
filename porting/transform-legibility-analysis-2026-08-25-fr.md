# Analyse : rendre `crates/transform` plus simple à lire pour un humain

**Date :** 2026-08-25
**Périmètre :** tout le crate `crates/transform` (150 fichiers `.rs`), avec `signal_fir/` comme
sujet principal. Analyse seulement — aucun changement de code n'est proposé pour un atterrissage
immédiat ; chaque expérience ci-dessous est une campagne indépendante et préservant le comportement.
**Statut :** proposé ; **E1 exécutée le 2026-08-25** sur la branche
`transform-legibility-e1` (commits `9fa67e0e` correction de vérité
loop_graph + amas mort, `41c83228` quarantaine des diagnostics, `a6bb1c6d`
passe des noms de code + gardes 6/7 de `structure-check` + ordre de lecture
— voir le journal du 2026-08-25). E2 et E3 restent proposées. E1 a aussi
fait émerger un suivi : la machinerie de chunk-driver `-vec` côté scalaire
(la branche vectorielle d'`emit_sample_loop` et la moitié chunking de
`loop_graph.rs`) est prouvablement inatteignable en production et peut être
supprimée sans changement de comportement (tâche séparée).
**Objectif :** identifier pourquoi le crate reste coûteux à lire pour un *humain* après les
décompositions de juin 2026 et le nettoyage R0–R9 de juillet 2026, et proposer trois expériences
de restructuration **indépendantes** qui réduisent le coût de lecture tout en gardant un **FIR
émis identique à l'octet ⇒ C/C++/WASM générés identiques ⇒ performance identique**.
**Jumeau anglais :**
[`transform-legibility-analysis-2026-08-25-en.md`](transform-legibility-analysis-2026-08-25-en.md)
(la version anglaise est canonique).
**Documents compagnons :**
[`signal-to-fir-transform-analysis-2026-06-20-en.md`](signal-to-fir-transform-analysis-2026-06-20-en.md)
(parcours étape par étape du pipeline ; toujours la meilleure référence « qu'est-ce qui se passe
dans quel ordre »),
[`transform-cleanup-documentation-factorization-plan-2026-07-19-en.md`](transform-cleanup-documentation-factorization-plan-2026-07-19-en.md)
(le nettoyage structurel R0–R9 exécuté, sur lequel cette analyse s'appuie),
[`delay-rs-simplification-experiment-2026-06-21-fr.md`](delay-rs-simplification-experiment-2026-06-21-fr.md) et
[`signal-prepare-simplification-experiment-2026-06-22-en.md`](signal-prepare-simplification-experiment-2026-06-22-en.md)
(les deux expériences de lisibilité précédentes, toutes deux implémentées ; même méthode appliquée
ici à l'échelle du crate).

---

## 0. Position dans la chaîne de compilation (rappel)

```
boxes ──► propagate ──► signals (+ UiProgram)
                            │
                            ▼
        ┌────────────────────────────────────────────────────────┐
        │ crates/transform                                        │
        │                                                         │
        │  signal_prepare ──► clk_env / hgraph / schedule ──►     │
        │    (staging)          (analyse)                         │
        │                       signal_fir ──► FIR                │
        │              (lowering scalaire + vectoriel certifié)   │
        └────────────────────────────────────────────────────────┘
                            │
                            ▼
              fir ──► codegen (C / C++ / WASM / Cranelift / FBC)
```

Le pipeline lui-même est bien documenté dans [`lib.rs`](../crates/transform/src/lib.rs) et dans
l'analyse du 2026-06-20 ; ce document ne le répète pas. L'unique point d'entrée de production est
[`compile_signals_to_fir_fastlane`](../crates/transform/src/signal_fir/mod.rs:615), piloté par le
builder `SignalFirRequest` ([mod.rs:481](../crates/transform/src/signal_fir/mod.rs:481)) — lui-même
issu d'une bonne correction de lisibilité du 2026-08-18 (cinq points d'entrée quasi identiques
fusionnés en une struct de requête ; le commentaire de doc y raconte l'histoire).

## 1. État mesuré (2026-08-25, `main-dev`)

Tous les nombres ont été mesurés à cette date ; les commandes de mesure sont données pour que les
tableaux puissent être re-dérivés après l'atterrissage de chaque expérience.

### 1.1 Échelle du crate

`transform` est désormais le **plus gros crate de l'espace de travail** (`python3
scripts/loc_report.py --by-crate`, basé sur cloc, lignes vides/commentaires exclus) :

| Crate | LOC effectives | LOC de test | Total |
|---|---:|---:|---:|
| **transform** | **34 628** | **15 681** | **50 309** |
| codegen | 32 717 | 11 839 | 44 556 |
| compiler | 11 536 | 18 208 | 29 744 |

Comptes bruts (`wc -l`, commentaires et lignes vides inclus) : 62 399 lignes sur 150 fichiers.

### 1.2 Où vivent les lignes (`wc -l` brut par sous-arbre)

| Sous-arbre | Lignes brutes | Part | Note |
|---|---:|---:|---|
| `signal_fir/vector/` | 29 915 | 48 % | pipeline vectoriel certifié (11 étages × model/build/check/tests) |
| fichiers racine de `signal_fir/` | 8 913 | 14 % | `mod.rs`, `loop_graph`, `cse`, `decoration_verify`, `recursion`, `pv_slice`, `shadow`, … |
| `signal_fir/module/` | 8 095 | 13 % | lowerer scalaire (14 fichiers) |
| `signal_fir/tests/` | 5 152 | 8 % | tests du chemin scalaire, déjà découpés par sujet |
| `signal_prepare/` | 3 026 | 5 % | staging + vérificateur |
| `schedule/` | 2 548 | 4 % | ordonnanceur générique `-ss` |
| `signal_fir/delay/` | 2 252 | 4 % | organisation de juin 2026, toujours saine |
| `hgraph/` + `clk_env/` | 2 437 | 4 % | étages d'analyse |

### 1.3 Échelle au niveau des fonctions — le constat principal

Un balayage par équilibre d'accolades sur `src/` compte **1 589 fonctions**, dont **69 dépassent
100 lignes et 21 dépassent 200 lignes**. Le cœur algorithmique du crate est concentré dans ces
fonctions :

| Lignes | Fonction | Emplacement |
|---:|---|---|
| 767 | `build_module` | [`module/build.rs:541`](../crates/transform/src/signal_fir/module/build.rs:541) |
| 643 | `verify_vector_plan` | [`vector/verify/check.rs:49`](../crates/transform/src/signal_fir/vector/verify/check.rs:49) |
| 585 | `build_vector_plan` | [`vector/plan/build.rs:105`](../crates/transform/src/signal_fir/vector/plan/build.rs:105) |
| 557 | `verify_prepared_signal` | [`signal_prepare/verify.rs:127`](../crates/transform/src/signal_prepare/verify.rs:127) |
| 536 | `build_fused_serial_groups` | [`vector/plan/fusion.rs:21`](../crates/transform/src/signal_fir/vector/plan/fusion.rs:21) |
| 447 | `verify_fused_serial_groups_after_plan` | [`vector/verify/fused_groups.rs:30`](../crates/transform/src/signal_fir/vector/verify/fused_groups.rs:30) |
| 345 | `lower_raw` | [`vector/lower/signal.rs:573`](../crates/transform/src/signal_fir/vector/lower/signal.rs:573) |
| 305 | `lower_vector_program_impl` | [`vector/lower/signal.rs:156`](../crates/transform/src/signal_fir/vector/lower/signal.rs:156) |
| 300 | `propagate_bra_adj` | [`module/bra.rs:429`](../crates/transform/src/signal_fir/module/bra.rs:429) |
| 289 | `compile_fastlane_inner` | [`signal_fir/mod.rs:648`](../crates/transform/src/signal_fir/mod.rs:648) |
| 277 | `ensure_guarded_block` | [`module/clocked.rs:487`](../crates/transform/src/signal_fir/module/clocked.rs:487) |
| 274 | `lower_signal` | [`module/core_lowering.rs:96`](../crates/transform/src/signal_fir/module/core_lowering.rs:96) |
| 261 | `infer_uncached` | [`clk_env/mod.rs:431`](../crates/transform/src/clk_env/mod.rs:431) |
| 260 | `materialize_action` | [`vector/assemble/materialize.rs:572`](../crates/transform/src/signal_fir/vector/assemble/materialize.rs:572) |
| 258 | `lower_proj` | [`module/arithmetic.rs:277`](../crates/transform/src/signal_fir/module/arithmetic.rs:277) |

19 fichiers hors tests dépassent encore 800 lignes brutes.

### 1.4 Les lowerers de production jumeaux

Les deux lowerers dispatchent déjà sur la vue typée
[`SigMatch`](../crates/signals/src/lib.rs:1128) (`match_sig`) — ce n'est donc *pas* du spaghetti sur arbre brut — mais le crate contient **deux
dispatchers signal→FIR de production complets** :

- scalaire : `lower_signal` (274 lignes) plus des helpers de branches répartis sur 9 des
  14 fichiers de `module/` ;
- vectoriel : `lower_raw` (345 lignes) plus des helpers dans `vector/lower/signal.rs`
  (2 339 lignes, le plus gros fichier du crate).

Il existe 210 sites d'appel `match_sig(` dans le crate. Les familles de branches sans état
(constantes numériques, binops, math unaire/binaire, casts, min/max/pow) sont émises **deux fois
avec des formes FIR identiques** ; le partage a déjà commencé pour exactement un élément
([`map_binop`](../crates/transform/src/signal_fir/module/arithmetic.rs) est importé par
`vector/lower/signal.rs:13`) mais s'arrête là.

Les deux structs d'état des lowerers restent grandes malgré l'extraction de sous-états de juin :

- `SignalToFirLower` ([`module/mod.rs:343`](../crates/transform/src/signal_fir/module/mod.rs:343)) :
  ~35 champs, dont 7 déjà regroupés en sous-états typés (le tableau à `mod.rs:330` les documente) ;
- `PureVectorLowerer` ([`vector/lower/signal.rs:48`](../crates/transform/src/signal_fir/vector/lower/signal.rs:48)) :
  ~40 champs **à plat** sans regroupement équivalent, plus un quadruplet de paramètres
  `(scope, sig, cache, active)` enfilé dans presque toutes les signatures de méthodes.

### 1.5 État de la documentation : d'excellentes cartes, des feuilles qui dérivent

Forces à préserver :

- les en-têtes de modules de [`clk_env`](../crates/transform/src/clk_env/mod.rs),
  [`hgraph`](../crates/transform/src/hgraph/mod.rs),
  [`schedule`](../crates/transform/src/schedule/mod.rs) et la table de carte des étages qui fait
  autorité dans [`vector/mod.rs`](../crates/transform/src/signal_fir/vector/mod.rs) sont un vrai
  matériau pédagogique ;
- les 10 types d'artefacts `Verified*` rendent la chaîne producteur/vérificateur visible dans le
  système de types ;
- `#![deny(missing_docs)]` ([`lib.rs:46`](../crates/transform/src/lib.rs:46)) garde chaque
  élément `pub` documenté.

Deux faiblesses mesurées :

**(a) Indirection par noms de code de plans.** ≈200 lignes de commentaires (grep sur
`P[0-9]\.[0-9]|R[0-9]|V[0-9]|S[0-9]|§[0-9]|Step 2[A-H]`) décrivent du code au présent dans les
coordonnées de plans de portage historiques (« roadmap P6, vector doc V2 », « P4.3b », « §4.8 »,
« Step 2A..2G », « S6 »). Le glossaire de `vector/mod.rs` § « Plan-codename glossary » atténue
cela pour le seul arbre vectoriel ; partout ailleurs le lecteur a besoin de l'historique de
`porting/` pour comprendre un commentaire de doc.

**(b) Dérive de vérité, une instance concrète.**
[`loop_graph.rs`](../crates/transform/src/signal_fir/loop_graph.rs:21) affirme encore *« Nothing
here is wired into scalar codegen yet, so it cannot affect existing output; the `dead_code`
allowance is removed when V3 starts populating it »* — mais
[`module/build.rs:991`](../crates/transform/src/signal_fir/module/build.rs:991) fait passer
**chaque tranche per-sample scalaire** par `LoopGraph` aujourd'hui. Supprimer le
`#![allow(dead_code)]` de niveau fichier (ligne 23) produit exactement 11 warnings : l'allowance
cache désormais un vrai amas mort — `LoopKind::Island`, `is_vectorizable`,
`len`/`is_empty`/`add_dep`, `loop_kind`, `LoopAssignment`, `loop_of`, `signal_value_children`,
`assign_loops`, `assign_one`, `name` — la moitié « affectation de boucles » du fichier, supplantée
par `vector/plan/`.

### 1.6 Surfaces de diagnostic mélangées à l'arbre de production

Deux modules d'observation pure vivent sans distinction à côté du code de production :

- [`pv_slice.rs`](../crates/transform/src/signal_fir/pv_slice.rs) (680 lignes, diagnostic
  pré-tranche P2) : consommé uniquement par `crates/compiler/tests/pv_vector_slice.rs` et des
  tests internes au crate ;
- [`shadow.rs`](../crates/transform/src/signal_fir/shadow.rs) (rapports de conformité
  d'ordonnancement) : consommé uniquement par `crates/compiler/tests/p3_shadow_mode.rs` et la
  variable d'environnement `FAUST_RS_SHADOW_REPORT` ; sa plomberie (`emission_order`,
  `emission_seen`, `shadow_report`) traverse `SignalToFirLower` et `SignalFirOutput`.

Un lecteur qui parcourt le chemin de production ne peut pas savoir, sans lire chaque en-tête,
quels voisins sont porteurs.

## 2. Diagnostic : ce qui coûte encore à un lecteur humain

Les campagnes de juin/juillet ont réglé l'histoire *au niveau des modules* : fichiers découpés
par sujet, étages cartographiés, tests séparés. Ce qui reste cher est un niveau en dessous et un
niveau au-dessus :

- **L1 — échelle des fonctions.** Les 21 fonctions de plus de 200 lignes sont là où vivent les
  vrais algorithmes, et à l'intérieur il n'y a aucune unité narrative nommée. `build_module`
  (767 lignes) est le cas le plus net : tout l'assemblage du module scalaire est une seule
  fonction.
- **L2 — duplication des lowerers jumeaux.** Deux dispatchers de production répètent branche par
  branche l'émission des feuilles sans état. Le coût est une double lecture et un vrai risque de
  divergence (un correctif appliqué à un seul chemin), et ce n'est *pas* une frontière
  d'assurance intentionnelle — la doctrine producteur/vérificateur de `vector/mod.rs` protège
  les vérificateurs, pas deux producteurs.
- **L3 — indirection par noms de code.** Les commentaires de doc parlent en coordonnées de
  plans ; le code décrit sa propre histoire au lieu de son comportement présent.
- **L4 — dérive de vérité.** Un en-tête périmé mesuré + un `allow` global cachant un amas mort
  (§1.5b). Chaque instance érode la confiance dans des en-têtes par ailleurs excellents.
- **L5 — mélange production/diagnostic.** §1.6.

## 3. Trois expériences de restructuration indépendantes

Chaque expérience est indépendamment atterrissable, neutre pour le FIR, et verrouillée
mécaniquement. Elles sont ordonnées de la moins chère à la plus chère ; le §5 donne le protocole
de migration commun.

### E1 — Documentation au présent et maintenance de la vérité (cible L3, L4, L5)

Risque FIR nul par construction (commentaires, déplacements, suppression de code mort).

1. **Noms sémantiques plutôt que noms de code.** Chaque commentaire de doc qui *explique un
   comportement* via un nom de code de plan est réécrit pour expliquer le comportement avec ses
   propres mots ; la provenance est conservée, mais reléguée à une ligne finale
   `Plan provenance:` (ou au glossaire par arbre, en étendant le modèle de `vector/mod.rs` à la
   racine du crate). Exemple : *« roadmap P2.3 per-clock-domain registry »* → *« un champ
   `IOTA`/`DSCounter` par domaine d'horloge (provenance : plan ondemand P2.3) »*.
2. **Garde mécanique.** Un vérificateur (étendant la famille existante de contrôles structurels
   d'`xtask`) rejette les motifs de noms de code dans le texte `///`/`//!` hors lignes
   `Plan provenance:` et sections de glossaire. Validé par une mutation rejetante avant
   d'atterrir (méthodologie des phases).
3. **Corrections de vérité.** Réécrire l'en-tête de `loop_graph.rs` pour décrire son rôle réel
   (scalaire + vectoriel) ; supprimer l'amas mort aux 11 warnings (ou déplacer ce dont
   `pv_slice` a vraiment besoin dans l'arbre de diagnostic) ; interdire les
   `#![allow(dead_code)]` de niveau fichier dans le crate via le même vérificateur.
4. **Quarantaine des diagnostics.** Déplacer `pv_slice` et `shadow` sous
   `signal_fir/diagnostics/` avec des ré-exports préservant les imports des tests de `compiler`,
   et une phrase d'en-tête chacun : *« observation seulement ; jamais sur le chemin de
   production »*.
5. **Ordre de lecture.** Ajouter une courte section *« Comment lire ce crate »* à `lib.rs`
   (ordre : `signal_prepare` → `clk_env` → `hgraph` → `schedule` → `module/` → carte des étages
   de `vector/mod.rs` → un étage dans l'ordre `model → build → check`), pour que les bons
   en-têtes existants deviennent une visite guidée.

### E2 — Décomposition en recette des fonctions surdimensionnées (cible L1)

Appliquer la méthode de juin un niveau plus bas : chaque fonction du tableau §1.3 devient un
court **orchestrateur qui se lit comme une table des matières** — une séquence linéaire de
fonctions de phase nommées, chacune avec un en-tête de contrat (entrées, sorties, invariant
maintenu). Pur refactoring d'extraction de fonctions ; aucun réordonnancement, aucun changement
de structure de données.

- **Ordre d'attaque :** d'abord le côté vérificateur (`verify_vector_plan`,
  `verify_prepared_signal`, `verify_fused_serial_groups_after_plan` — le plus sûr : une erreur de
  vérificateur échoue fermé et le corpus golden attrape les changements d'admission), puis les
  constructeurs de plans, puis les lowerers, avec `build_module` en dernier (le plus gros et le
  plus central).
- **Regroupement de paramètres.** Là où l'extraction créerait des helpers à ≥5 arguments,
  regrouper d'abord l'état mutable enfilé : dans le lowerer vectoriel, le triplet
  `(scope, cache, active)` devient une struct `LowerCursor<'_>` — même forme d'emprunts, un seul
  nom. Refléter le tableau de sous-états de juin en regroupant les ~40 champs à plat de
  `PureVectorLowerer` dans le même genre de sous-états typés que ceux déjà documentés à
  `module/mod.rs:330` (tables, sous-modules, instantanés de contrôle externe, UI).
- **Garde mécanique.** Un contrôle de longueur maximale de fonction à cliquet : démarre au pire
  actuel (767), chaque atterrissage abaisse le cliquet, finit à 200 pour les fonctions hors
  tests. Même validation par mutation rejetante que E1.2.
- **Cible :** 21 fonctions > 200 lignes → 0 ; 69 > 100 → moins de 30.

### E3 — Une grammaire de feuilles, deux ordonnanceurs (cible L2)

Extraire les familles d'émission **sans état** partagées par les deux lowerers de production dans
un module unique (`signal_fir/leaf_emit.rs`) : constantes numériques, binops (politique de typage
incluse), opérations math unaires et binaires, casts, min/max/pow. Chaque fonction est libre de
contexte — `fn emit_binop(store, ty, op, lhs: FirId, rhs: FirId) -> FirId` — et chaque branche de
dispatcher devient un appel. Cela termine ce que `map_binop` a commencé, et répète au niveau des
signaux le motif qui a déjà réussi au niveau des backends (le cœur d'émission commun de la
famille C, documenté dans `porting/`, les 7 divergences closes).

- **Garde de périmètre (dedans) :** une famille de branches n'entre dans `leaf_emit` qu'après
  qu'un diff a prouvé que les deux chemins émettent la forme FIR identique pour des identifiants
  d'opérandes identiques *aujourd'hui*.
- **Garde de périmètre (dehors) :** tout ce qui touche l'état, le placement, le cache, les
  régions, l'UI, les tables, les délais, la récursion — et `select2` tant que l'identité n'est
  pas prouvée.
- **Garde de doctrine :** on ne partage ici que du *vocabulaire côté producteur* (exactement
  comme `FirBuilder` lui-même). Les vérificateurs continuent de re-dériver leurs propres
  preuves ; la frontière producteur/vérificateur §3.2 de `vector/mod.rs` est intacte.
- **Cible :** les deux dispatchers se réduisent à leurs branches réellement spécifiques au
  chemin ; un correctif de branche sans état ne peut plus atterrir sur un seul chemin.

### Interactions entre les expériences

E1 est indépendante des deux autres. E2 et E3 touchent les deux mêmes fichiers de dispatch ; la
règle de composition est la sérialisation par fichier : **dans un fichier donné, faire atterrir
l'extraction E2 avant la substitution E3** (des branches courtes rendent les diffs E3 lisibles).
Il n'existe aucun autre couplage — tout sous-ensemble des trois peut atterrir, dans n'importe
quel ordre entre fichiers.

## 4. Ce qu'il ne faut PAS simplifier (garde-fous)

Ces propriétés ressemblent à de la duplication ou du pédantisme pour un lecteur neuf, mais elles
sont porteuses :

1. **La duplication producteur/vérificateur** (en-tête de `vector/mod.rs`) : un vérificateur
   n'appelle jamais son producteur, ne réutilise jamais un cache de producteur, n'accepte jamais
   un résultat attendu dérivé du producteur. C'est la frontière d'assurance — la dédupliquer
   serait une régression même à diff FIR nul.
2. **L'ordonnancement déterministe** : `BTreeMap`/itération triée par clé partout où un ordre
   d'émission est dérivé (la garde de déterminisme d'émission existe précisément parce qu'une
   « simplification » en `HashMap` a un jour réordonné la sortie).
3. **Le repli vectoriel qui échoue fermé** avec les codes stables `FRS-VEC-FALLBACK-*`, et la
   distinction statut/mode effectif (`VectorPipelineStatus` vs `VectorEffectiveMode`).
4. **`#![deny(missing_docs)]`** et les gardes docs/layout de R9.
5. **L'exactitude bit à bit scalaire/vectoriel** et les ancres de parité C++ (en-têtes de
   provenance citant `8eebea429`).

## 5. Protocole de migration (commun aux trois expériences)

Par atterrissage (un commit = une étape nommable) :

1. reconstruction propre — ne jamais faire confiance aux `.ir`/états de target en cache (piège
   connu de faux verts) ;
2. **diff FIR golden à l'octet** sur le corpus (`dsp/` + corpus `tests/impulse-tests`) à travers
   la matrice de modes touchée par l'étape (scalaire, `-vec`, `-ss 0..3`, `-ec`, `-os`,
   `-double`, les deux modes `--table-init`) — identique à l'octet, sinon l'étape est un défaut ;
3. suite complète `cargo test -p transform` + espace de travail ;
4. oracle d'impulsions (133/133 × 8 backends) — la certification structurelle n'est pas une
   preuve numérique ;
5. suite de certification : 98 certifiés / 0 erreur sur les 16 modes, inchangé ;
6. contrôle ponctuel `make bench` / `make compile-bench` quand une étape touche un chemin chaud
   (E3 surtout) ;
7. chaque **nouveau vérificateur/garde** (E1.2, le cliquet d'E2) est validé par une mutation
   rejetante avant d'atterrir.

Ordre de campagne suggéré : **E1 → E2 → E3**, chacune sur sa propre branche, entrée de journal
par atterrissage.

## 6. État final attendu (mesurable)

| Métrique | Aujourd'hui | Cible |
|---|---:|---:|
| fonctions hors tests > 200 lignes | 21 | 0 |
| fonctions hors tests > 100 lignes | 69 | < 30 |
| lignes de commentaires citant des noms de code hors sections de provenance | ≈200 | 0 |
| `#![allow(dead_code)]` de niveau fichier | 1 | 0 |
| affirmations d'en-tête périmées (mesurées) | 1 | 0 (gardé) |
| implémentations d'émission de feuilles sans état | 2 | 1 |
| modules d'observation pure distinguables par leur chemin | non | oui (`diagnostics/`) |

Le total de LOC ne devrait baisser que modestement (≈1–2 k lignes : amas mort, branches de
feuilles dédupliquées). C'est voulu : l'objectif de ces expériences est le **temps de lecture**,
pas le nombre de lignes — le comptage de lignes a déjà son propre rapport
(`scripts/loc_report.py`), et le nettoyage de juillet a montré que déplacer des lignes est
facile, tandis que leur faire raconter leur histoire au présent est la partie qui paie.
