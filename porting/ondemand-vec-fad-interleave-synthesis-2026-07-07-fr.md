# Synthèse : domaines d'horloge (ondemand/US/DS), mode vectoriel, FAD/RAD et primitive `interleave` pour le calcul spectral

Date : 2026-07-07

Statut : proposé — remplace le
[roadmap du 2026-06-10](ondemand-vec-fad-implementation-roadmap-2026-06-10-en.md)
comme surface de suivi ; les trois documents d'analyse restent les
références techniques par sujet.

Version anglaise : [ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md](ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md)
(même contenu ; maintenir les deux en phase lors des amendements).

Famille de documents (conventions de renvoi inchangées) :

- **plan §N** : [ondemand-clock-domains-analysis-port-plan-2026-06-10-en.md](ondemand-clock-domains-analysis-port-plan-2026-06-10-en.md) — analyse C++ des domaines d'horloge + plan de port en 8 étapes ;
- **cohabitation §N** : [ondemand-fad-rad-cohabitation-2026-06-10-en.md](ondemand-fad-rad-cohabitation-2026-06-10-en.md) — FAD/RAD × domaines d'horloge (phases A/B/C) ;
- **vector doc §N** : [vector-mode-analysis-port-plan-2026-06-10-en.md](vector-mode-analysis-port-plan-2026-06-10-en.md) — port de `-vec` (V1–V6) et composition avec les domaines (D1/D2) ;
- **roadmap PN** : le roadmap consolidé du 2026-06-10 (phases P0–P9), dont ce
  document reprend et met à jour l'ordre d'atterrissage.

Nouveau par rapport au 2026-06-10 :

1. **État de faust-rs re-vérifié au 2026-07-07** (§2) : rien de P0–P9 n'a
   atterri, mais quatre refactors intermédiaires changent les fichiers-cibles
   et réduisent le coût de plusieurs phases.
2. **Le volet spectral** (§4–§5) : analyse de la primitive `interleave`
   (sérialisation temps ↔ largeur de frame autour d'`ondemand`) qui rend le
   *frame-rate processing* (STFT, FFT par frame, loss spectrale
   différentiable) exprimable en Faust pur — et son insertion dans le plan
   (track S1–S5, §7).
3. **Une interaction de chantier** : le port de mémoïsation de `propagate`
   en cours (plan du 2026-07-04) doit inclure l'environnement d'horloge dans
   sa clé de cache, sinon il réintroduit le bug P0.3 (§2.3).

Référence C++ inchangée : branche `master-dev-ocpp-od-fir-2-FIR19`, commit
`8eebea429` pour la machinerie clockée ; upstream `master` pour `-vec` de
base ; **aucune référence C++** pour FAD/RAD × domaines ni pour
`interleave` — faust-rs définit la sémantique, l'oracle est numérique.

## 1. Les enjeux en une page

Quatre chantiers, un seul cœur sémantique :

1. **Domaines d'horloge** (`ondemand`/`upsampling`/`downsampling`). La
   moitié avant (syntaxe → boxes → eval → propagation → graphe de signaux
   clocké typé) est **déjà à parité** dans faust-rs (plan §6.1). La moitié
   arrière n'existe pas : inférence d'environnement d'horloge, graphe de
   dépendances hiérarchique, blocs gardés dans le FIR, temps local par
   domaine (`IOTA`/`DSCounter`), émission backend. C'est le port
   proprement dit (plan §7, roadmap P0–P3).
2. **FAD/RAD × domaines** — la motivation applicative : apprentissage
   in-graph à cadence de contrôle (`ondemand(ad.fit_adam …)`), adaptation
   déclenchée par événement, gradients décimés, contrôleurs DDSP à cadence
   de frame (cohabitation §2). Deux faits structurent tout :
   - **La falaise de correction** : aujourd'hui `fad` à travers une
     frontière produit des tangentes *silencieusement nulles*
     (cohabitation §4), masqué par l'échec `FRS-SFIR-0004` en aval. Le
     jour où `signal_prepare` accepte les nœuds clockés, une boucle
     d'apprentissage compilera et ne convergera jamais. Le diagnostic FAD
     bruyant doit atterrir **dans le même change set** que le fix
     `signal_prepare` (P0 indivisible).
   - La différentiation **commute avec chaque opérateur de frontière**
     tant que l'horloge ne dépend pas de la seed (cohabitation §5) :
     `fad` strictement *dans* un domaine ne demande **zéro code AD
     nouveau** (Phase A) ; le franchissement exact de frontière est une
     réécriture structurelle « augmenter une fois » (Phase B) ; RAD exige
     une tape consciente des horloges (Phase C), sauf cas à taux constant
     où la transposée LPTV du chemin YOLO suffit.
3. **Mode vectoriel** (`-vec`) : inexistant dans faust-rs, désactivé sur la
   branche de recherche C++. faust-rs le porte **une seule fois** (site de
   lowering unique `signal_fir`, vector doc §4) comme `LoopGraph`
   déterministe (V1–V6), puis le compose avec les domaines via les **îlots
   scalaires** (D1) : chaque bloc OD/US/DS devient un nœud de boucle sériel
   dont l'interface est exactement la glue `TempVar`/`PermVar` — bit-exact,
   sans rejet d'option. L'invariant CSE partagé (« ne jamais hisser à
   travers une frontière de région ») est construit une fois (P2) pour les
   deux consommateurs.
4. **Calcul spectral** (`interleave`, nouveau) : Faust ne sait faire de la
   FFT qu'en régime *sliding* (recalcul par échantillon, `analyzers.lib`).
   Le régime *frame-rate* — celui de la STFT, du phase vocoder, des loss
   spectrales DDSP — n'est pas exprimable. L'analyse (§4) montre que
   `ondemand` fournit déjà presque tout : il manque une **seule brique**,
   la sortie zéro-stuffée (`↑₀`, duale de la décimation), et le cœur FFT
   spatial (`an.fftb`) se réutilise tel quel. Bonus décisif : une STFT en
   Faust pur est automatiquement différentiable — c'est l'infrastructure
   des loss spectrales in-graph.

## 2. État de faust-rs au 2026-07-07 (vérifié sur `main-dev`)

### 2.1 Rien du roadmap P0–P9 n'a atterri

Chaque point re-vérifié dans les sources :

| Constat | Preuve | Phase concernée |
|---|---|---|
| L'environnement d'horloge est encore traversé comme un signal : `Clocked(x, y)` partage un bras avec `Seq`/`ZeroPad` et visite `x` | `crates/transform/src/signal_prepare/verify.rs:257` | P0.1 |
| `make_clock_env` laisse toujours `slotenv`/`path` à nil (bug d'unicité d'instance) | `crates/propagate/src/engine.rs:1086` | P0.2 |
| Le fallback silencieux `zero_tangent` attrape toujours toute la glue (`Seq`/`Clocked`/`TempVar`/`PermVar`/`ZeroPad`/OD/US/DS) via `_ =>` | `crates/propagate/src/forward_ad.rs:1075` | P0.4 |
| Le rejet RAD dit toujours `kind: "other"` sans nommer la construction | `crates/propagate/src/reverse_ad.rs:331` | P0.4 |
| Pas de module d'inférence d'horloge (`clk_env` absent de `crates/transform/src/`) | arborescence | P1 |
| Pas de `ComputeMode` dans `SignalFirOptions`, pas de plomberie `-vec`/`-vs`/`-lv` | `crates/transform/src/signal_fir/mod.rs:119` | P6 (V1) |
| Aucune trace d'`interleave` (parser, boxes, signals) | grep | track S |

### 2.2 Ce qui a changé depuis le 2026-06-10 : quatre refactors qui déplacent les cibles

Aucun n'implémente une phase, mais tous changent les fichiers que le plan
du 2026-06-10 citait, et deux réduisent réellement le coût :

1. **`signal_prepare` restructuré** (`9841595b`, `e18d20f9`, `7b8d118e`) :
   désormais `crates/transform/src/signal_prepare/{mod,verify,rewrites}.rs`
   avec un driver `Staging` typé. Le fix P0.1 vise maintenant
   `verify.rs` (sortir `Clocked` du bras partagé ligne 257 et ne plus
   visiter le premier enfant) et `rewrites.rs` pour les canonicalisations.
2. **`delay.rs` éclaté** (`aa94c747` → `6558a85e`) :
   `signal_fir/delay/{manager,plan,context}` + émission par stratégie via
   `DelayKind`, marche unifiée `plan_delays`. Les points d'intégration
   « IOTA par domaine » (P2.3/P3.1) et « stratégies de délai vectorielles »
   (V4) atterrissent sur cette structure par-stratégie — nettement plus
   accueillante que le monolithe décrit dans le vector doc §4.
3. **`SignalToFirLower` décomposé** (W9, `df1786a3` → `6366cbab`) :
   sous-structures extraites (`ModuleSections`, `PlacementInfo`,
   `UiLoweringState`, `NameGen`, `RadReverseState`, `BraState`…). Le
   refactor de régions P2.2 — remplacer l'accumulateur plat
   `sample_phases` par un arbre de régions — part d'un état déjà
   décomposé ; le risque « gros refactor sur gros struct » a baissé d'un
   cran.
4. **Cœur d'émission C-family partagé** (`5c6db8d7` + `068fbaa6`, les 7
   drifts clos) : l'émission structurée des blocs gardés (P3.2) et des
   drivers de chunk (V5) s'écrit **une fois** pour c/cpp au lieu de deux.

Conséquence : les tailles estimées du roadmap restent valables sauf P2
(M–L → plutôt M) et P3.2/V5 (émission une fois au lieu de deux).

### 2.3 Interaction avec le chantier mémoïsation de `propagate` (en cours)

Le plan
[cpp-propagate-eval-memoization-port-plan-2026-07-04-en.md](cpp-propagate-eval-memoization-port-plan-2026-07-04-en.md)
va introduire un memo de résultats dans `propagate_in_slot_env` — le trou
principal identifié. **Contrainte croisée** : en C++ la clé de mémoïsation
de `propagate` inclut le `clockenv`
(plan §3.2, [propagate.cpp:918-929]). Si le memo Rust atterrit avec une
clé sans environnement d'horloge, le même box propagé sous deux domaines
rendra le même signal — c'est exactement le bug que P0.3 devait auditer,
sauf qu'il serait *créé* par le port de perf au lieu d'être hérité. À
inscrire comme exigence du plan de mémoïsation dès maintenant, même si les
domaines ne sont pas encore exploitables en aval : la propagation, elle,
construit déjà les graphes clockés.

## 3. Rappel condensé du socle (renvois)

Pour ne pas dupliquer les analyses, seul l'essentiel opérationnel :

- **Architecture en phases** (plan §3–§5) : propagation marque les
  frontières (`TempVar`/`double_clocked` en entrée, `PermVar`+`Clocked` en
  sortie, `Seq(OD, permvar)` comme contrainte d'ordre) → l'inférence
  assigne chaque signal à son domaine (calcul d'horloge monotone, point
  fixe de Kleene, domaines strictement emboîtés) → le graphe hiérarchique
  `Hgraph` partitionne → l'ordonnancement produit `Hsched` → la génération
  matérialise blocs gardés, temps locaux et variables de frontière. Verdict
  du plan §5.3 : **porter les algorithmes 1:1, moderniser les
  représentations** (arène `ClockDomain` à jeton d'unicité au lieu du tuple
  cons ; side-map au lieu de propriété d'arbre ; toposort déterministe
  unique ; diagnostics `FRS-` structurés).
- **FAD/RAD** (cohabitation §5–§7) : les tangentes vivent dans les mêmes
  groupes récursifs que les primals, donc primal/tangente sont co-clockés
  *par construction* ; la règle de franchissement est « augmenter le bloc
  une fois » (une seule exécution du corps par tick de feu, même env
  d'horloge réutilisé) ; l'adjoint de `ondemand` est un
  integrate-and-dump gaté (hold ↔ accumulation-au-feu, zero-pad ↔
  décimation), d'où tape consciente des horloges pour le cas général et
  transposée LPTV pour les taux constants.
- **Mode vectoriel** (vector doc §2, §5–§6) : `LoopGraph` déterministe
  (arène `LoopId`, critère `needSeparateLoop` porté verbatim, buffers de
  chunk, deux layouts de délai bloc, drivers `-lv 0|1`), îlots scalaires
  pour les blocs clockés (D1), vectorisation des intérieurs US/DS à
  facteur littéral (D2). Politique RAD : les modules à boucle temps-inverse
  forcent le mode scalaire tant que la fenêtre TBPTT sous chunking n'est
  pas tranchée.

## 4. Le volet spectral : analyse de la primitive `interleave`

Source : la conversation d'analyse `RUST/INTERLEAVED.md` (dérivation
`gather`/`scatter` en termes de `↓`/`↑`/`@`/`od`), formalisée et corrigée
ici. Aucune référence C++ : c'est une extension faust-rs (et une
proposition remontable à upstream).

### 4.1 Le besoin et le trou

`analyzers.lib` sait déjà faire une FFT — mais en régime **sliding** :
`an.fft(N) = si.cbus(N) : an.c_bit_reverse_shuffle(N) : an.fftb(N)` est un
circuit purement spatial (aucun `@`, `~` ni `mem` dans `fftb` ; twiddles =
constantes de compilation) recalculé **à chaque échantillon** —
O(N log N) *par sample*. Le régime frame-rate standard (une transformée
par hop, O(N log N) *par frame*) n'est pas exprimable : il manque le
cadencement. `ondemand` est exactement ce cadencement ; ce qui manque est
la conversion **temps ↔ largeur de frame**.

### 4.2 La décomposition : trois étages, un seul manquant

```
interleave(N, FX) = serialize_in(N) : od_periodique(FX) : serialize_out(N)
```

avec une horloge périodique `H` de période `N` et de **phase N−1**
(tick aux instants `t ≡ N−1 (mod N)`, i.e. quand la fenêtre vient d'être
remplie) :

1. **`serialize_in(N) = par(i, N, _@(N-1-i))`** — du **sucre pur** sur les
   délais existants. Au tick `t = kN−1`, la ligne `i` porte
   `x((k−1)N + i)` : la fenêtre contiguë, dans l'ordre. Point de design
   crucial (montage « (b) » de l'analyse) : `serialize_in` opère **hors**
   du `od`, en temps plein — à l'intérieur du domaine décimé, `@1`
   vaudrait un tick = N samples et la fenêtre ne serait plus contiguë.
2. **`od(FX)`** — réutilisé tel quel. La décimation `↓H` des N lignes
   compacte les ticks : dans le temps décimé, `FX` voit une vraie frame
   par pas. C'est le port de base (P3) sans rien de plus.
3. **`serialize_out(N)`** — la brique manquante. La sortie d'`ondemand`
   est un **hold** (`PermVar`, sample-and-hold) : pendant tout un bloc de
   N samples, les N lignes tiennent *en parallèle* les N valeurs de la
   frame. Pour reconstruire un flux temporel il faut un **démultiplexage
   temporel** : la ligne `j` ne doit être non nulle qu'à son slot. Le
   `↑`-hold ne le fournit pas ; il faut le dual manquant, l'upsampling à
   zéros `↑₀` — qui est aussi la **vraie transposée** de la décimation au
   sens du produit scalaire (le hold ne l'est pas), ce qui en fait la
   bonne brique pour les gradients (§4.5).

Attention au vocabulaire : le nœud existant `SigZeroPad(x, H)`
(`crates/signals/src/lib.rs:1191`) est la glue d'**entrée** d'`upsampling`
(valeur sur la dernière itération interne, 0 sinon) — apparenté mais
distinct du `↑₀` de **sortie** dont il est question ici.

### 4.3 Trois options pour `↑₀`, et la recommandation

**Option A — sucre de bibliothèque (recommandée en V1).** Pour une horloge
*booléenne périodique*, l'indicateur de feu est l'horloge elle-même, lisible
au taux externe. Or la sortie `Seq(OD, PermVar(…))` tient la valeur fraîche
dès le tick de feu (le `Seq` garantit bloc-avant-lecture dans le même
tick). Donc :

```faust
up0(H, y) = y * (H != 0);          // zéro-stuff = hold masqué par l'horloge
serialize_out(N) = par(j, N, up0(H) : @(j)) :> _;
```

Zéro primitive nouvelle, zéro nœud signal nouveau : `interleave` devient
une **définition de bibliothèque** au-dessus du port de base. Restriction
assumée : horloge booléenne (pas OD entier ni US, où « feu » ≠ valeur de
H au taux externe) — ce qui couvre tout le cas STFT.

**Option B — nœud signal dédié** (`SigZeroVar`, dual de `SigPermVar` :
valeur au feu, 0 sinon). Sémantique plus propre, transposée exacte
native pour RAD (P8 : l'adjoint de `↑₀` est la décimation, sans passer
par la règle produit + horloge non-différentiable), coût : un nœud à
travers tout le pipeline (typage, prepare, inférence, lowering,
backends). À ne faire que si l'option A montre un vrai problème de
gradient ou de perf.

**Option C — primitive `interleave` monolithique** : rejetée, elle
dupliquerait la machinerie qu'`ondemand` fait déjà.

### 4.4 Latence et convention de phase — dérivation, à verrouiller par la table N=2

Avec la convention phase `N−1` et le retard de sortie `@(j)` (ligne `j`,
0-indexée), la dérivation donne pour `interleave(N, identité)` :
l'échantillon `x((k−1)N + i)` entre à `t = (k−1)N + i`, est exposé sur la
ligne `i` au tick `t = kN−1`, ressort (option A) à `t = kN−1+i` — latence
**constante N−1**, c'est-à-dire `interleave(N, id) = @(N−1)`, et pour
`N = 2` on retrouve bien `mem`. Le `2N−1` évoqué dans l'analyse initiale
venait de la sémantique hold-avec-décalage du `↑` du papier ; l'option A
(valeur fraîche au tick même via `Seq`) l'évite. **Jalon de
verrouillage** : la table déroulée sample-par-sample pour `N=2`
(colonnes `t, x, H, lignes serialize_in, feu, FX=id, up0, sortie`) comme
test structurel + test runtime `interleave(N, si.bus(N)) == @(N−1)` pour
plusieurs N — c'est le S1 du plan (§7).

Recouvrement (hop < N) : la même construction fait de l'**overlap-add
gratuit** — horloge de période `hop`, `serialize_in` inchangé (fenêtres
recouvrantes), et en sortie la sommation `:>` des lignes zéro-stuffées
retardées additionne naturellement les frames qui se chevauchent. La
condition COLA sur la fenêtre reste à la charge de l'utilisateur
(vérifiable, plus tard, comme théorème — hors scope compilateur).

### 4.5 FFT, différentiabilité, et positionnement honnête

- **Réutilisation totale du cœur** : `an.c_bit_reverse_shuffle(N)` +
  `an.fftb(N)` sont déjà spatiaux et testés ;
  `fft_framed(N) = interleave(N, an.rtocv(N) : an.fftb(N))` ne réécrit
  rien — on change le harnais de cadencement, pas le DSP. Pour
  l'**analyse seule** (loss spectrale), `serialize_out` est inutile : les
  bins tenus par les `PermVar` au taux de frame sont directement
  consommables ; la resynthèse (phase vocoder) seule exige `serialize_out`
  + OLA.
- **Oracle intégré** : la sliding FFT de la lib *est* la référence — aux
  ticks d'alignement, `interleave(rtocv : fftb)` doit produire le même
  spectre que `rtocv : fft` sliding. Pas besoin de FFT externe.
- **Différentiabilité** : `fftb` = routage + arithmétique réelle → déjà
  dans le fragment FAD/RAD. Mais une loss spectrale réaliste
  (`fad(loss ∘ |STFT| ∘ dsp(θ), θ)`) **traverse la frontière** (les
  paramètres et le signal entrent par `serialize_in`, domaine externe) →
  exige la **Phase B** (P5). En Phase A on peut déjà différencier ce qui
  vit entièrement au taux de frame. Côté RAD : l'horloge périodique est à
  **taux constant**, donc la STFT est linéaire périodiquement variante —
  la **transposée LPTV** du chemin YOLO
  ([yolo-linearize-once-rad-analysis-2026-05-21-en.md](yolo-linearize-once-rad-analysis-2026-05-21-en.md))
  couvre `rad` à travers la STFT *avant* la tape générale P8. Détail
  numérique table-stakes : `∂|X|` est singulier en 0 → epsilon dans la
  magnitude (`sqrt(R²+I²+ε)`).
- **Positionnement** (à tenir tel quel en présentation) : la FFT
  différentiable + loss spectrale est un acquis DDSP/JAX/PyTorch depuis
  2020 — en régime *offline, batch, GPU, reverse-mode*. La contribution
  faust-rs n'est pas la différentiabilité de la FFT, c'est son intégration
  dans le modèle d'exécution **synchrone temps réel** : adaptation
  in-graph au fil de l'audio, source unique multi-backend, sans runtime
  ML. La démo décisive : un biquad dont les coefficients descendent le
  gradient d'une loss spectrale **en live**, compilé en un plugin sans
  dépendance.
- **Mode vectoriel** : l'`ondemand` périodique de l'STFT devient un îlot
  scalaire sous D1 (correct, sériel) ; et c'est le **candidat idéal de
  D2** — facteur littéral, intérieur `fftb` entièrement stateless → SIMD
  au taux de frame.

### 4.6 Risques propres au volet spectral

1. **Coût de compilation** : `fftb(N)` déplié = O(N log N) nœuds +
   `route` de bit-reversal à N paires ; `serialize_in(N)` = N lignes à
   délais O(N). Pour N = 1024–4096, stress réel du pattern-matcher et de
   la taille du code généré. Jalonner : N=4 → 64 → 1024 avec mesures.
2. **Convention de phase non verrouillée** avant S1 — tout le reste du
   track S en dépend (alignement analyse/resynthèse).
3. **Option A et horloges non booléennes** : restriction documentée, avec
   diagnostic si `up0` est utilisé sous OD entier/US.
4. **Délais de `serialize_in` au taux externe** : N−1 lignes de délai vers
   le même signal — vérifier que le partage de lignes à retards multiples
   (stratégies `delay/`) produit bien *un* buffer, pas N.

## 5. Vue d'ensemble des dépendances (mise à jour)

Le graphe P0–P9 du roadmap reste valable ; le track spectral S s'y
raccorde ainsi :

```
P0 ──→ P1 ──┐
            ├──→ P3 ──→ P4 ──→ P5 ──┬──→ P8 ──→ P9 (LPTV, TBPTT)
P2 ─────────┤        │              │
            │        ├──→ S1 ──→ S2 ──→ S3 ──→ S4 (S4 exige P5)
            └──→ P6 ─┤              │
                     └──→ P7 ───────┴──→ P9 (D2) ──→ S5
```

- **S1–S3 ne dépendent que de P3** (lowering scalaire OD + backends
  C/C++) : dès que le port de base tourne, le volet spectral démarre —
  sans attendre FAD Phase B ni le mode vectoriel.
- **S4** (STFT différentiable) exige P5 (Phase B, franchissement de
  frontière).
- **S5** (perf) exige P7+P9-D2 (vectorisation d'intérieur d'îlot) et,
  pour `rad`-STFT, le chemin LPTV (P9) ou P8.

## 6. Plan d'implémentation P0–P9 actualisé

Les contenus détaillés (checklists) du roadmap du 2026-06-10 restent la
référence ; ci-dessous ce qui **change** par phase — cibles de fichiers
post-refactors, et ajustements.

### P0 — Gardes & fondations (S–M, **indivisible**, à faire en premier)

Inchangé sur le fond (roadmap §3), cibles réactualisées :

- **P0.1** : sortir `Clocked` du bras partagé de
  `crates/transform/src/signal_prepare/verify.rs:257` (ne jamais visiter
  le premier enfant) ; ajouter les bras manquants
  `Seq`/`TempVar`/`PermVar`/`ZeroPad`/`OD`/`US`/`DS` dans `verify.rs` et
  auditer `rewrites.rs` + occurrences/CSE ; rejet `FRS-SFIR` propre dans
  `signal_fir` (« ondemand not lowered yet »).
- **P0.2** : arène `ClockDomain { parent, kind, clock, instance }` à la
  place du tuple cons ; corrige `make_clock_env`
  (`crates/propagate/src/engine.rs:1086`). Test : deux instances
  structurellement identiques → domaines distincts.
- **P0.3** : audit de la clé de cache — **fusionné avec le chantier
  mémoïsation** (§2.3) : l'exigence « clé ⊇ clock_env » entre dans le
  plan `cpp-propagate-eval-memoization-port-plan-2026-07-04-en.md`, avec
  le test « même box sous deux domaines → signaux distincts ».
- **P0.4** : remplacer le `_ => zero_tangent(sig)` de
  `crates/propagate/src/forward_ad.rs:1075` par un bras explicite pour la
  glue → erreur `FRS-PROP` structurée ; nommer la construction dans le
  rejet RAD (`reverse_ad.rs:331`). Snapshots des quatre lignes du tableau
  cohabitation §4.

### P1 — Analyses de domaines (M) : inchangé

Inférence (`R_PROJ`/`R_CLOCKED`/`R_CD`/`R_SEQ`/`R_COMPOSITE`, point fixe
Jacobi, side-map `SigId → ClockDomainId`) + `Hgraph`/`Hsched` (partition
auditée, toposort DFS déterministe). Nouveau module
`crates/transform/src/clk_env.rs` (ou crate dédié). Détails roadmap §4.

### P2 — Infrastructure de régions dans `signal_fir` (M, revu à la baisse)

La décomposition W9 de `SignalToFirLower` et l'éclatement de `delay/`
font une bonne partie du travail préparatoire :

- **P2.1** note de design régions (inchangée) : arbre `Region`, règle de
  visibilité unique (« une valeur calculée dans R n'est réutilisable que
  dans R et ses descendants ; la réutilisation inter-régions passe par du
  stockage nommé ») ; décision vocabulaire FIR (réutiliser
  `If`/`SimpleForLoop`/`Block` existants, défaut confirmé par le vector
  doc §4).
- **P2.2** refactor diff-free : remplacer l'accumulateur `sample_phases`
  (dans `signal_fir/module/build.rs`) et le CSE par-bucket
  (`cse.rs`, `placement.rs`) par l'arbre de régions instancié avec une
  seule `SampleLoop` (+ la boucle temps-inverse comme région sœur).
  **Acceptation : zéro diff sur tous les goldens.**
- **P2.3** classes de stockage : `PermVar` → champs struct clearés,
  `TempVar` → locaux de la région parente, `IOTA`/`DSCounter` par
  `ClockDomainId` — s'intègre dans `signal_fir/delay/{manager,plan}.rs`.

### P3 — Lowering scalaire OD/US/DS + premiers backends (L)

Inchangé (roadmap §6) avec deux mises à jour :

- **P3.2** : l'émission structurée des blocs gardés s'écrit dans le
  **cœur C-family partagé** — une implémentation pour c et cpp, plus les
  rejets nommés pour chaque autre backend.
- **P3.4** : harnais différentiel contre le binaire de la branche
  (`8eebea429`, faust 2.84.3), corpus du roadmap §6 inchangé. Ajouter dès
  ce stade le fixture « horloge booléenne périodique de période N » —
  c'est la brique du track S.

### P4 / P5 — FAD Phases A et B (S–M / M) : inchangés

Roadmap §7–§8. Rappels : Phase A = corpus uniquement (zéro code AD
nouveau) avec les six familles de cas d'usage ; Phase B = règles duales
sur la glue + `OD_aug` mémoïsé une fois par bloc source + interaction
`suppress_fad`/`ExpandAfterRec` testée + relâchement du diagnostic P0.4.

### P6 / P7 — Mode vectoriel V1–V6 puis îlots D1 (L / M) : inchangés

Roadmap §9–§10. Mise à jour de cible : les stratégies de délai
vectorielles (V4) s'ajoutent comme variantes de `DelayKind` dans
`signal_fir/delay/` ; l'émission V5 profite du cœur C-family. Piste
parallèle possible dès la fin de P2.

### P8 / P9 — RAD Phase C et optimisations : inchangés

Roadmap §11–§12. S'y ajoute (P9) : la transposée LPTV couvre `rad` à
travers l'STFT à hop constant (S4/S5) avant la tape générale.

## 7. Track S — le volet spectral (nouveau)

### S1 — Sémantique et convention de phase (S ; dépend de P3.1–P3.2)

- [ ] Table N=2 déroulée sample-par-sample (fixture structurelle) fixant :
      phase de l'horloge (`t ≡ N−1 mod N`), retards de sortie `@(j)`,
      convention « valeur fraîche au tick de feu ».
- [ ] Test runtime : `interleave(N, si.bus(N)) == @(N−1)` pour
      N ∈ {2, 4, 16} (latence constante, identité à retard près).
- [ ] Décision `↑₀` **option A** (sucre `up0(H, y) = y * (H != 0)`)
      documentée, avec la restriction horloge-booléenne et son
      diagnostic ; l'option B (`SigZeroVar`) consignée comme replis avec
      ses critères de déclenchement (gradient RAD natif, perf).
- [ ] Court doc de design dans `porting/` (ou rustdoc) si des écarts à la
      présente analyse apparaissent.

### S2 — Bibliothèque `interleave` (S ; dépend de S1)

- [ ] Définitions lib (faust) : `serialize_in(N)`, `up0`,
      `serialize_out(N)` (variante somme/OLA), `interleave(N, FX)`,
      horloge périodique `frame_clock(N)` (phase N−1) et
      `frame_clock(N, hop)` pour le recouvrement.
- [ ] Fixture impulse-tests : `interleave(N, id)`, `interleave` avec
      délai interne (IOTA local), recouvrement hop = N/2 (OLA, fenêtre
      COLA simple).
- [ ] Vérification du partage des lignes de délai de `serialize_in`
      (un buffer, pas N — stratégie `delay/`).

### S3 — Jalon FFT (M ; dépend de S2)

- [ ] `fft_framed(4) = interleave(4, an.rtocv(4) : an.fftb(4))` validé
      contre la sliding FFT de `analyzers.lib` aux ticks d'alignement
      (l'oracle est dans la lib) ; latence bout-en-bout mesurée et
      confrontée à S1.
- [ ] Montée en N : 64 puis 1024, avec mesure du temps de compilation et
      de la taille du code généré (stress pattern-matcher/CSE) ;
      seuils/alertes consignés.
- [ ] Analyse seule sans `serialize_out` (bins tenus au taux de frame)
      comme mode documenté.

### S4 — STFT différentiable (M ; dépend de S3 **et P5**)

- [ ] `fad` à travers `interleave` (les seeds traversent
      `serialize_in`) : gradient d'un twiddle rendu variable dans
      `fftb(4)` vs différences finies.
- [ ] Loss spectrale magnitude avec epsilon (`sqrt(R²+I²+ε)`) ;
      convergence d'un paramètre de filtre sur une loss `|STFT|` à taux
      de frame (le cas cohabitation §2.4 restructuré).
- [ ] Démo flagship : biquad adaptatif à loss spectrale **en temps
      réel**, compilé C++ sans dépendance — l'artefact de positionnement
      (§4.5).
- [ ] `rad` : snapshot du rejet nommé tant que ni LPTV ni P8 ne couvrent
      le cas.

### S5 — Performance spectrale (M ; dépend de P7 + P9-D2)

- [ ] L'îlot STFT sous `-vec` : bit-exact vs scalaire (D1), puis
      intérieur `fftb` vectorisé au taux de frame (D2 — candidat idéal :
      facteur littéral, corps stateless).
- [ ] `rad`-STFT par transposée LPTV (hop constant) ; mesure tape/coût.
- [ ] Comparaison de débit sliding vs framed vs framed-vec (l'argument
      O(N log N)/hop chiffré).

## 8. Ordre d'atterrissage plat (mono-flux)

1. **P0** gardes & fondations — change set indivisible ; l'exigence
   clock-env s'inscrit en parallèle dans le plan mémoïsation (§2.3)
2. **P1** inférence + `Hgraph`/`Hsched`
3. **P2** régions (design, refactor diff-free, classes de stockage)
4. **P3** lowering scalaire + backends C/C++ + SR/UI + harnais
   différentiel — puis P3.5 backends en différé
5. **P4** FAD Phase A (corpus)
6. **S1–S2** sémantique `interleave` + bibliothèque — *dès que P3 tient ;
   peut précéder P5*
7. **P5** FAD Phase B (augmentation de bloc)
8. **S3** jalon FFT framed vs sliding
9. **S4** STFT différentiable + démo temps réel
10. **P6** mode vectoriel V1–V6 — *second flux possible dès la fin de P2*
11. **P7** îlots D1
12. **P8** RAD Phase C
13. **P9 + S5** optimisations (D2, LPTV, hoisting, TBPTT, perf spectrale)

Avec deux flux : {P0, P1} ∥ {P2}, puis {P3, P4, S1–S3, P5, S4} ∥ {P6},
jonction à P7, queue {P8, P9, S5}.

## 9. Risques consolidés (delta vs roadmap §13)

1. **Falaise FAD** (inchangé, toujours ouverte) : P0 indivisible ; aucun
   état intermédiaire ne doit compiler des gradients silencieusement nuls.
2. **Mémoïsation × clock_env** (nouveau, §2.3) : à verrouiller dans le
   plan de mémoïsation *avant* que son implémentation n'atterrisse.
3. **Référence C++ instable** (inchangé) : parité épinglée à `8eebea429` ;
   re-sync délibéré et journalisé.
4. **Compile-time FFT** (nouveau, §4.6) : jalonner N=4 → 1024 avec
   mesures ; c'est aussi un banc d'essai pour les optimisations du
   pattern-matcher.
5. **Convention de phase `interleave`** (nouveau) : rien du track S
   au-delà de S1 tant que la table N=2 n'est pas verrouillée.
6. **Fenêtre TBPTT sous `-vec`** (inchangé) : force-scalaire pour les
   modules à boucle temps-inverse jusqu'à décision (P9).
7. **Backend interp** (inchangé) : chemins bloc gardé + boucles de chunk
   = le plus gros écart backend (P3.5, P6.6).

## 10. Validation (surfaces d'oracle)

| Sujet | Oracle |
|---|---|
| Domaines d'horloge (base) | Différentiel vs binaire branche `8eebea429` (impulse-tests, style `cpp_signal_differential`) |
| Mode vectoriel (base) | **Bit-exact** scalaire vs `-vec` intra-faust-rs + différentiel vs upstream `master` `-vec -lv 0|1` |
| `-vec` × domaines (D1/D2) | Bit-exact scalaire vs `-vec` intra-faust-rs (pas de référence upstream) |
| FAD/RAD × domaines | Différences finies (harnais `fad_recursive_runtime.rs` / `rad_runtime.rs` / `block_reverse_ad.rs`) — pas de référence C++ |
| `interleave` / FFT framed | Identité à retard près (`@(N−1)`), puis **sliding FFT de `analyzers.lib` comme référence** aux ticks d'alignement |
| STFT différentiable | Différences finies + convergence mesurée de la démo temps réel |
