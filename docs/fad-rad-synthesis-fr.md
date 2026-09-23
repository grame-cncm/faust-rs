---
title: "Note de synthèse: usages de FAD et RAD dans faust-rs"
author: "OpenAI Codex"
date: "2026-07-22"
---

# Usages de FAD et RAD dans `faust-rs`

Version anglaise: [fad-rad-synthesis-en.md](fad-rad-synthesis-en.md) (même
contenu; maintenir les deux versions synchronisées).

Ce document présente FAD et RAD du point de vue de l'utilisateur Faust. Il ne
suppose pas de connaissance préalable de la différenciation automatique.

Dans un programme Faust classique, un DSP transforme des entrées audio et des
contrôles en sorties audio. Avec FAD et RAD, le DSP peut aussi produire des
dérivées: par exemple "comment la sortie change si ce gain augmente ?", ou
"dans quelle direction faut-il déplacer ce coefficient pour réduire l'erreur ?".

Deux primitives sont disponibles:

```faust
fad(expr, seeds)
rad(expr, seeds)
```

Ce sont des extensions de `faust-rs`: le compilateur C++ Faust de référence
utilisé par le projet ne reconnaît pas actuellement `fad` ni `rad`.

Dans la suite:

- le **primal** est la valeur normale du signal `expr`;
- une **seed** est une variable par rapport à laquelle on différencie;
- une **tangente** est une dérivée produite par FAD;
- un **gradient** est une dérivée produite par RAD;
- une **perte** est un signal scalaire que l'on cherche à minimiser, souvent
  `err * err`.

## 1. Comment lire les sorties

### FAD: dérivées locales en mode direct

Si `expr` produit `M` signaux et `seeds` en produit `N`, FAD émet, pour chaque
sortie primale, ses `N` tangentes:

```text
fad(expr, (s0, s1, ...)) =
    [p0, dp0/ds0, dp0/ds1, ...,
     p1, dp1/ds0, dp1/ds1, ...,
     ...]
```

L'arité de sortie vaut donc `M * (1 + N)`. Pour une expression scalaire, on
retrouve `[expr, d(expr)/ds0, d(expr)/ds1, ...]`.

Exemple:

```faust
x = hslider("x", 1, 0, 10, 0.01);
y = hslider("y", 2, 0, 10, 0.01);
process = fad(x * y, (x, y));
```

Sorties:

```text
[x*y, y, x]
```

FAD est bien adapté quand la dérivée doit rester dans le graphe Faust: mise à
jour récursive, Newton, filtre auto-apprenant, contrôle adaptatif, etc.

### RAD: gradients d'une perte ou d'une somme de sorties

Pour une expression scalaire:

```text
rad(loss, (p0, p1, ...)) =
    [loss, d(loss)/d(p0), d(loss)/d(p1), ...]
```

Pour une expression à plusieurs sorties, RAD donne les gradients de la somme des
sorties primales. En pratique, on utilise donc souvent RAD sur une perte
scalaire déjà construite:

```faust
err = target - model;
loss = err * err;
process = rad(loss, (p0, p1));
```

RAD est intéressant quand on a une perte scalaire et plusieurs paramètres à
ajuster.

Pour un corps feed-forward, RAD effectue une passe inverse symbolique. Pour un
corps avec retards ou récursion, il utilise `BlockReverseAD`: le primal est joué
en avant sur le bloc `compute(count)` courant, puis l'adjoint est balayé en
arrière avec un état terminal nul à la fin du bloc. Les sorties de gradient sont
alors des contributions par échantillon à sommer sur ce bloc, et non des
scalaires déjà réduits ni un gradient à horizon infini. Consommé dans le
graphe -- une récursion d'adaptation qui lit `rad(loss(p), p) : !, _` à
l'échantillon qui le produit -- le balayage ne voit que cet échantillon: à
travers une récursion il renvoie le terme direct, l'état passé tenu fixe, là
où FAD transporte la dérivée à travers la récursion; pour un corps
feed-forward, les deux coïncident (section 11).

## 2. Gain auto-apprenant avec FAD

Exemple illustratif, fondé sur le cas de régression versionné
[`fad_recursive_local_projection.dsp`](../tests/corpus/fad_recursive_local_projection.dsp).

Cas d'usage: un DSP apprend un gain inconnu en comparant sa sortie à une cible.
Le gain estimé est stocké dans une récursion Faust, et FAD calcule la dérivée de
la perte par rapport à cette estimation.

```faust
import("stdfaust.lib");

target_gain = hslider("gain", 0.5, 0, 1, 0.01);
input = 1.0;
true_value = input * target_gain;

learned_gain = loop ~ _
with {
    loop(prev_gain) = next_gain
    with {
        rate = 0.01;

        learned_value = input * prev_gain;
        loss = (true_value - learned_value) * (true_value - learned_value);

        grad = fad(loss, prev_gain) : !, _;
        next_gain = prev_gain - rate * grad;
    };
};

process = true_value, (learned_gain : hbargraph("learned_gain", 0, 1));
```

Ce que montre l'exemple:

- la variable apprise est un état récursif Faust;
- la perte est calculée dans le DSP;
- FAD fournit `d(loss)/d(prev_gain)`;
- la descente de gradient est elle-même écrite en Faust.

C'est le plus petit exemple utile d'apprentissage "in-graph": il n'y a pas
besoin d'une boucle Python ou d'un runtime externe pour mettre à jour le
paramètre.

## 3. Identification de filtre résonant avec deux paramètres

Exemple de conception illustratif; le contrat multi-seed et les gradients
récursifs sont couverts séparément par
[`fad_multi_seed.dsp`](../tests/corpus/fad_multi_seed.dsp) et
[`fad_recursive_local_projection.dsp`](../tests/corpus/fad_recursive_local_projection.dsp).

Cas d'usage: un filtre modèle apprend à suivre un filtre cible. Les paramètres
appris sont la fréquence `f` et le facteur de qualité `q`.

```faust
import("stdfaust.lib");

process = no.noise : train ~ (_, _) : (!, !, _);

train(f_prev, q_prev, input) = f_next, q_next, model_out
with {
    f = select2(f_prev == 0.0, f_prev, 1000.0);
    q = select2(q_prev == 0.0, q_prev, 1.0);

    target_f = hslider("Target Freq", 1200, 20, 20000, 1);
    target_q = hslider("Target Q", 2.0, 0.1, 10.0, 0.01);

    target = input : fi.resonlp(target_f, target_q, 1.0);
    model_out = input : fi.resonlp(f, q, 1.0);
    err = target - model_out;

    diffs = fad(input : fi.resonlp(f, q, 1.0), (f, q)) : !, _, _;
    df = diffs : _, !;
    dq = diffs : !, _;

    raw_grad_f = -err * df;
    raw_grad_q = -err * dq;

    grad_f = max(-1.0, min(1.0, raw_grad_f));
    grad_q = max(-0.1, min(0.1, raw_grad_q));

    m_f = grad_f : si.smooth(0.9);
    v_f = (grad_f * grad_f) : si.smooth(0.999);
    m_q = grad_q : si.smooth(0.9);
    v_q = (grad_q * grad_q) : si.smooth(0.999);

    f_next = max(20.0, min(20000.0, f - 2.0 * (m_f / (sqrt(v_f) + 1e-3))))
        : hbargraph("Learned Freq", 20, 20000);

    q_next = max(0.1, min(10.0, q - 0.01 * (m_q / (sqrt(v_q) + 1e-3))))
        : hbargraph("Learned Q", 0.1, 10.0);
};
```

Ce que montre l'exemple:

- `fad(..., (f, q))` donne deux sensibilités en un seul appel;
- les gradients peuvent être lissés, bornés et normalisés comme des signaux DSP;
- l'optimiseur peut ressembler à Adam/RMSProp, mais rester entièrement dans
  Faust;
- les contraintes physiques ou numériques, ici `20..20000 Hz` et `0.1..10`,
  sont appliquées directement dans la mise à jour.

Ce type de patch est utile pour l'identification de système: on définit un
modèle interprétable, on observe une cible, puis le DSP ajuste ses paramètres
pour réduire l'erreur.

## 4. Biquad auto-apprenant à cinq coefficients

Cet exemple de conception exécutable complète le biquad adaptatif avec RAD de
[`rad_tbptt_biquad1.dsp`](../tests/corpus/rad_tbptt_biquad1.dsp). La bibliothèque
locale au projet [`optimizers.lib`](../libraries/optimizers.lib), utilisée
ci-dessous (préfixe `op`), est versionnée avec `faust-rs`; le fixture du corpus
[`opt_descend_lion_biquad_reflection.dsp`](../tests/corpus/opt_descend_lion_biquad_reflection.dsp)
en est la version sans bibliothèque standard, vérifiée par la suite de tests.
Concaténer les blocs Faust de cette section et compiler le programme obtenu
avec `-I libraries`.

Cas d'usage: apprendre les cinq coefficients d'un biquad
`b0, b1, b2, a1, a2` pour imiter une cible manipulée par l'utilisateur.

Le modèle audio est compact:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");

biquad_model(b0, b1, b2, a1, a2, audio) =
    fi.tf2(b0, b1, b2, a1, a2, audio);
```

Les paramètres cibles peuvent être exposés comme des sliders:

```faust
t_b0 = vslider("[1] Cible b0", 0.1, -2.0, 2.0, 0.001) : si.smooth(0.99);
t_b1 = vslider("[2] Cible b1", 0.2, -2.0, 2.0, 0.001) : si.smooth(0.99);
t_b2 = vslider("[3] Cible b2", 0.1, -2.0, 2.0, 0.001) : si.smooth(0.99);
t_a1 = vslider("[4] Cible a1", -1.0, -1.90, 1.90, 0.001) : si.smooth(0.99);
t_a2 = vslider("[5] Cible a2", 0.4, -0.90, 0.90, 0.001) : si.smooth(0.99);
```

Le modèle appris n'expose pas `a1, a2` directement: il apprend deux
coefficients de réflexion `k1, k2` dans `(-1, 1)` et les transforme par
`a1 = k1 (1 + k2)`, `a2 = k2`. Cette application est une bijection sur le
triangle de stabilité `|a2| < 1`, `|a1| < 1 + a2`: chaque filtre intermédiaire
est stable, ce que des bornes rectangulaires sur `a1, a2` ne peuvent pas
garantir. La perte s'écrit comme une fonction Faust ordinaire des cinq
paramètres, fermée sur l'excitation et la cible:

```faust
bruit = no.pink_noise;
target = biquad_model(t_b0, t_b1, t_b2, t_a1, t_a2, bruit);

modele_appris(b0, b1, b2, k1, k2) = biquad_model(b0, b1, b2, a1, a2, bruit)
with {
    a1 = op.poles_from_reflection(k1, k2) : _, !;
    a2 = op.poles_from_reflection(k1, k2) : !, _;
};
perte(b0, b1, b2, k1, k2) = op.mse(modele_appris(b0, b1, b2, k1, k2), target);
```

Le coeur de l'apprentissage est une descente 5D sur cette perte. Un seul
moteur Lion, sur une vitesse d'apprentissage à décroissance exponentielle,
sert les cinq paramètres: Lion avance de `±lr` dans la direction du signe de
son moment, quelle que soit l'échelle de chaque gradient, donc zéros et pôles
n'ont pas besoin de vitesses distinctes:

```faust
lion = op.lion_g(op.lr_exp(0.0002, 0.000002, 30000.0), 0.9, 0.99);
reset = button("[6] Reset");

opts = op.descend_5D(
    perte,
    lion, lion, lion, lion, lion,
    -2.0, 2.0,
    -2.0, 2.0,
    -2.0, 2.0,
    -0.999, 0.999,
    -0.999, 0.999,
    0.0, 0.0, 0.0, 0.0, 0.0,
    reset
);
```

On extrait ensuite les cinq paramètres appris, on ramène les coefficients de
réflexion aux pôles, on reconstruit le modèle appris et on définit un
`process` complet:

```faust
b0 = opts : _, !, !, !, !;
b1 = opts : !, _, !, !, !;
b2 = opts : !, !, _, !, !;
k1 = opts : !, !, !, _, !;
k2 = opts : !, !, !, !, _;
a1 = op.poles_from_reflection(k1, k2) : _, !;
a2 = op.poles_from_reflection(k1, k2) : !, _;

modele = modele_appris(b0, b1, b2, k1, k2);

process = target, modele, b0, b1, b2, a1, a2;
```

`descend_5D` factorise le motif suivant:

```faust
grads(p1, p2, p3, p4, p5) =
    fad(perte(p1, p2, p3, p4, p5), (p1, p2, p3, p4, p5)) : !, _, _, _, _, _;
```

un seul appel `fad` sur la perte, dont les cinq tangentes sont les gradients
`dperte/dp1 ... dperte/dp5`, puis un moteur de mise à jour par paramètre et une
projection sur les bornes. Rendu avec `faustprobe` en double précision, les
cinq coefficients sont à moins de `1e-5` de la cible après 300 000
échantillons.

Ce que montre l'exemple:

- FAD n'est pas limité à un seul slider;
- les optimiseurs peuvent être factorisés en bibliothèque Faust;
- la perte est une expression Faust quelconque (une perte robuste ou
  énergétique se brancherait de la même façon);
- la stabilité d'un filtre IIR s'obtient mieux par reparamétrisation que par
  des bornes.

La forme d'origine de cet exemple, `optimize_5D` avec des moteurs `rmsprop` et
des bornes rectangulaires sur les pôles, compile et converge toujours avec la
version 0.5.0 de la bibliothèque.

## 5. Newton pour équation implicite

Exemple illustratif de composition des deux sorties d'un FAD mono-seed.

Cas d'usage: résoudre une équation implicite de type analogique. On cherche
`y` tel que:

```text
E(y) = y - tanh(x - fb*y) = 0
```

Newton a besoin de `E(y)` et de `E'(y)`. FAD calcule automatiquement la dérivée
de l'erreur par rapport à l'hypothèse courante `y`.

```faust
import("stdfaust.lib");

circuit_error(x, fb, y) = y - ma.tanh(x - fb * y);

newton_step(x, fb, y) = y - (err / den)
with {
    err = circuit_error(x, fb, y);
    den = fad(circuit_error(x, fb, y), y) : !, _;
};

solve_circuit(x, fb) = 0.0 : seq(i, 5, newton_step(x, fb));

process = _ <: _, solve_circuit(feedback)
with {
    feedback = hslider("Analog_Feedback", 2.0, 0.0, 10.0, 0.01);
};
```

Ce que montre l'exemple:

- FAD évite d'écrire à la main la dérivée d'une équation non linéaire;
- le solveur reste un DSP Faust pur;
- plusieurs pas de Newton peuvent être déroulés avec `seq`;
- le signal original et le signal résolu peuvent être écoutés côte à côte.

Ce motif est pertinent pour les modèles analogiques, les saturations en boucle
de feedback, les approximations de circuits et les solveurs zéro-delay.

## 6. Contrôle actif de bruit avec FxLMS

Cet exemple exécutable utilise un contrôleur à un coefficient et un modèle de
chemin secondaire du premier ordre.

Cas d'usage: adapter un coefficient de contrôle pour minimiser le bruit
résiduel mesuré après un chemin secondaire. C'est le schéma classique FxLMS,
mais la dérivée est obtenue par FAD.

```faust
import("stdfaust.lib");

clamp(lo, hi, x) = min(hi, max(lo, x));
secondaryPath(x) = fi.lowpass(1, 1200, x);

process(ref, dist) = (loop ~ _) : !, _, _, _
with {
    mu = hslider("Mu", 0.001, 0.000001, 0.05, 0.000001);
    reset = button("Reset");
    filtered_ref = secondaryPath(ref);

    loop(w_prev) = w_next, err, y, w_prev
    with {
        y = w_prev * ref;
        err = dist + secondaryPath(y);

        sensitivity = fad(w_prev * filtered_ref, w_prev) : !, _;
        grad_w = 2.0 * err * sensitivity;

        updated = clamp(-2.0, 2.0, w_prev - mu * grad_w);
        w_next = select2(reset, updated, 0.0);
    };
};
```

Ce que montre l'exemple:

- le chemin secondaire physique et la récursion canonique restent hors de
  l'expression différentiée;
- FAD calcule la sensibilité du contrôleur à partir de la référence filtrée;
- l'erreur mesurée complète le gradient FxLMS échantillon par échantillon;
- le coefficient adaptatif reste borné;
- le bouton `Reset` remet l'apprentissage à zéro.

Ce cas est plus proche d'un usage audio industriel: annulation de bruit,
correction adaptative, compensation de chemin acoustique ou réglage automatique
d'un contrôleur.

## 7. Régression gain + biais pilotée par l'hôte avec RAD

Source d'inspiration: [`tests/corpus/rad_gain_bias_train.dsp`](../tests/corpus/rad_gain_bias_train.dsp).

Cas d'usage: le DSP calcule les dérivées, mais l'hôte accumule le gradient sur
un bloc et met à jour les sliders entre deux appels `compute`.

```faust
gain = hslider("gain", 1.0, -4.0, 4.0, 0.001);
bias = hslider("bias", 0.0, -4.0, 4.0, 0.001);

process = rad(gain * _ + bias, (gain, bias));
```

Sorties:

```text
[out, d(out)/d(gain), d(out)/d(bias)]
```

Pour une perte MSE côté hôte:

```text
err[n] = out[n] - target[n]
grad_gain = sum_n 2 * err[n] * d(out[n])/d(gain)
grad_bias = sum_n 2 * err[n] * d(out[n])/d(bias)
```

Ce que montre l'exemple:

- RAD donne directement un gradient par paramètre;
- le DSP reste simple et stateless du point de vue de l'optimiseur;
- l'hôte peut choisir le batch, le learning rate, le clipping, l'optimiseur et
  la politique de mise à jour;
- ce modèle convient aux plugins, aux tests offline ou à l'apprentissage piloté
  par une application.

## 8. Notch adaptatif avec RAD

Source d'inspiration:
[`tests/corpus/rad_adaptive_notch_omega.dsp`](../tests/corpus/rad_adaptive_notch_omega.dsp).

Cas d'usage: identifier la fréquence dominante d'un signal et déplacer un notch
vers cette fréquence.

```faust
omega = hslider("omega", 1.0, 0.01, 3.0, 0.0001);

notch(xn, xn1, xn2) = xn - 2.0 * cos(omega) * xn1 + xn2;
process = rad(notch, omega);
```

Le filtre correspond à:

```text
H(z) = 1 - 2*cos(omega)*z^-1 + z^-2
```

Il place deux zéros sur le cercle unité, à l'angle `omega`. Si l'hôte minimise
la puissance de sortie:

```text
loss = mean(y*y)
```

alors la descente de gradient pousse `omega` vers la fréquence la plus présente
dans l'entrée.

Ce que montre l'exemple:

- RAD est pratique quand un seul paramètre contrôle une structure analytique;
- l'hôte peut gérer les retards `x[n-1]`, `x[n-2]` et les fournir comme entrées;
- le DSP expose la dérivée `d(y)/d(omega)`;
- l'application peut faire un LMS classique en dehors du DSP.

## 9. LMS FIR à plusieurs taps avec RAD

Source d'inspiration:
[`tests/corpus/rad_tbptt_lms_fir3.dsp`](../tests/corpus/rad_tbptt_lms_fir3.dsp).

Cas d'usage: apprendre les coefficients d'un FIR trois taps qui imite une cible
cachée.

```faust
h0_star = 0.5;
h1_star = 0.3;
h2_star = -0.2;
lr = 0.02;

noise = lcg * 4.656612873077393e-10
with { lcg = +(12345) ~ *(1103515245); };

x  = noise;
x1 = x  : mem;
x2 = x1 : mem;
y_target = h0_star * x + h1_star * x1 + h2_star * x2;

taps = loop ~ (_, _, _)
with {
    loop(h0, h1, h2) = h0n, h1n, h2n
    with {
        y_pred = h0 * x + h1 * x1 + h2 * x2;
        err = y_target - y_pred;
        loss = err * err;

        g0 = rad(loss, h0) : !, _;
        g1 = rad(loss, h1) : !, _;
        g2 = rad(loss, h2) : !, _;

        h0n = max(-4.0, min(4.0, h0 - lr * g0));
        h1n = max(-4.0, min(4.0, h1 - lr * g1));
        h2n = max(-4.0, min(4.0, h2 - lr * g2));
    };
};

h0 = taps : _, !, !;
h1 = taps : !, _, !;
h2 = taps : !, !, _;

process = (y_target - (h0 * x + h1 * x1 + h2 * x2)) <: _, _;
```

Ce que montre l'exemple:

- RAD peut être utilisé à l'intérieur d'une boucle d'adaptation Faust;
- chaque coefficient reçoit son gradient par rapport à la perte;
- les coefficients appris sont bornés;
- le signal de sortie peut être le résidu, donc l'utilisateur entend directement
  la convergence;
- le corps est feed-forward (les entrées retardées ne dépendent pas des
  coefficients), donc l'horizon d'un échantillon du balayage dans le graphe ne
  perd rien: le gradient est exact. `optimizers.lib` emballe ce motif pour `N`
  coefficients dans `descend_N_rad` et `lsq_N_rad`, un balayage par
  échantillon pour les `N` dérivées.

Ce motif couvre les usages classiques de filtrage adaptatif: identification
d'impulsion, égalisation adaptative, annulation d'écho simplifiée, prédiction
linéaire et calibration de réponse.

## 10. Apprentissage par trame avec `ondemand`

Exemples exécutables ; les boucles cadencées sont dans
[`optimizers.lib`](../libraries/optimizers.lib) 0.6.0 et l'enrobage de trame
dans [`interleave.lib`](../libraries/interleave.lib). Compiler avec
`-I libraries`.

Cas d'usage: adapter les paramètres une fois par trame plutôt qu'une fois par
échantillon — pour économiser du CPU, pour avancer sur un gradient de
mini-lot, ou parce que la perte vit sur un spectre. `ondemand(C)` n'exécute `C`
que sur les échantillons où sa première entrée, l'horloge, est non nulle, et
maintient les sorties entre deux; dans le corps, une récursion avance une fois
par tir. `fad` se compose avec lui: la différenciation dans un bloc est prise
en charge (vérifiée contre des différences finies), une graine peut entrer
dans le bloc comme entrée explicite, et une dérivée ne traverse jamais seule
une frontière d'horloge (`rad` à travers une frontière est refusé).

La forme bibliothèque garde la perte et `fad` à cadence audio, moyenne le
gradient sur la trame avec `op.frame_mean` (une moyenne exacte, remise à zéro
par l'horloge), et prend le pas dans un bloc `ondemand`, si bien que l'état du
moteur avance une fois par trame:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
il = library("interleave.lib");
x = no.noise;
target = 0.7 * x;
loss(g) = op.mse(g * x, target);
g = op.descend_1D_clocked(il.frame_clock(64), loss, op.sgd_g(0.5), -4.0, 4.0, 0.0, 0.0);
process = g, target - g * x;
```

Rendu avec `faustprobe`, le gain vaut `0,700000` dès 4 000 échantillons, avec
un pas SGD par trame de 64 échantillons. Placer toute une boucle dans un bloc,
`(il.frame_clock(64), x, target) : ondemand(learn)` avec
`learn(xi, ti) = op.descend_1D(\(g).(op.mse(g * xi, ti)), ...)`, fonctionne
aussi: tout tourne alors en temps de tir sur les entrées du bloc (gain à
`1e-6` de la cible après 312 pas).

La forme DDSP par trame: la trame de `N` échantillons entre dans le bloc par
ses entrées, la perte est calculée sur sa FFT, et l'optimiseur avance une fois
par trame dans le bloc:

```faust
il = library("interleave.lib");
an = library("analyzers.lib");
si = library("signals.lib");
no = library("noises.lib");
op = library("optimizers.lib");
N = 8;
target_energy = 4.0;
cmag(re, im) = sqrt(re * re + im * im + 0.000000001);
magsum = par(m, N, cmag) :> _;
// Le bloc reçoit les N échantillons de la trame comme arguments nommés (un
// opérateur de trame à entrées `_` libres verrait ses entrées dupliquées à chaque usage).
learn(x0, x1, x2, x3, x4, x5, x6, x7) =
    op.descend_1D(loss, op.adam_g(0.02, 0.9, 0.999, 1e-8), 0.01, 10.0, 1.0, 0.0)
with {
    // perte spectrale de la trame mise à l'échelle par g : (somme |X_k| - cible)^2
    loss(g) = (x0, x1, x2, x3, x4, x5, x6, x7) : par(i, N, *(g) : (_, 0)) : an.fft(N) : magsum : -(target_energy) <: _ * _;
};
process = no.noise : il.serialize_in(N) : (il.frame_clock(N), si.bus(N)) : ondemand(learn);
```

Le gain appris se stabilise à `0,34`; l'optimum des moindres carrés pour cette
excitation vaut `0,340`. Deux règles sur lesquelles reposent les exemples: un
corps reçoit les signaux extérieurs comme entrées explicites (une définition
référencée à l'intérieur est instanciée à nouveau dans le temps propre du
corps, ce n'est pas le signal extérieur), et un opérateur de trame à entrées
`_` libres reçoit des arguments nommés, sinon ses entrées sont dupliquées à
chaque usage. `ma.SR` n'est pas
adapté dans `ondemand`. Les primitives sont décrites dans
[ondemand-note-fr.md](ondemand-note-fr.md); un parcours pas à pas est la
section 11 de
[optimizers-ddsp-tutorial-fr.md](../libraries/optimizers-ddsp-tutorial-fr.md).

## 11. Quand choisir FAD ou RAD ?

Utiliser **FAD** quand:

- le gradient doit être consommé immédiatement dans le DSP;
- le nombre de paramètres est petit ou moyen;
- le patch contient une mise à jour récursive écrite en Faust;
- on veut une pente locale, par exemple pour Newton ou pour une non-linéarité;
- on écrit une bibliothèque d'optimiseurs Faust comme `descend_1D`,
  `lsq_3D` ou `lm_2D` d'`optimizers.lib`.

Utiliser **RAD** quand:

- on part d'une perte scalaire;
- on veut plusieurs gradients pour une même perte;
- l'hôte peut accumuler les gradients sur un bloc;
- on fait de la régression, du LMS, un notch adaptatif ou un apprentissage
  paramétrique piloté depuis l'extérieur;
- on veut éviter de multiplier les calculs quand le nombre de paramètres
  augmente;
- beaucoup de paramètres partagent une perte dans le graphe: les boucles à bus
  `_rad` d'`optimizers.lib` font un balayage par échantillon, exact pour un
  modèle feed-forward et égal au terme direct (état passé tenu fixe) à travers
  une récursion.

Il faut mesurer les programmes représentatifs avant de supposer un avantage de
performance. Sur un FIR à 16 coefficients appris dans le graphe, la boucle à
bus `rad` compile en 1 182 instructions d'interpréteur contre 3 777 pour `fad`
(0,04 s contre 0,10 s pour 200 000 échantillons), 4 129 contre 28 891 à 64
coefficients (0,13 s contre 1,32 s); sur les petits graphes, les passes de
simplification et de partage des sous-expressions peuvent rapprocher les deux
coûts.

## 12. Limites pratiques à garder en tête

Ces primitives ne transforment pas Faust en framework de deep learning général.
Elles sont surtout utiles pour des DSP paramétriques, interprétables et
fortement contraints.

Points pratiques:

- une seed doit être donnée explicitement;
- les seeds sont reconnues par identité de Signal IR après lowering; une
  expression seulement équivalente algébriquement n'est pas résolue
  automatiquement;
- les paramètres appris doivent souvent être bornés pour éviter les explosions;
- les gradients doivent souvent être lissés, normalisés ou clippés;
- pour les filtres récursifs, il faut respecter les zones de stabilité;
- pour RAD sur des signaux temporels, raisonner par blocs ou par mise à jour
  hôte reste plus simple; l'horizon inverse est le bloc `compute(count)` courant
  pour une sortie publique, et l'échantillon courant quand le gradient est
  consommé dans le graphe (terme direct à travers une récursion);
- FAD possède les règles duales pour les blocs valides
  `ondemand`/`upsampling`/`downsampling`, avec une horloge opaque; les tests
  d'intégration actuels couvrent surtout les formes FAD autour et à l'intérieur
  de `ondemand` (section 10). RAD à travers une frontière de domaine d'horloge
  reste refusé; un `rad` dont l'expression et les graines vivent dans un même
  corps `ondemand` tourne dans ce domaine (les entrées d'un bloc et les
  constantes étrangères sont des feuilles du balayage, l'enveloppe d'horloge
  est traversée);
- les règles symboliques ne couvrent pas toutes les familles de signaux: FAD
  conserve le primal avec une tangente nulle aux frontières non modélisées,
  tandis que RAD refuse explicitement les familles dures comme les tables
  mutables, les soundfiles et les fonctions étrangères non reconnues;
- les lectures de tables read-only utilisent une pente par différence finie
  symétrique, pas une dérivée analytique du contenu de table;
- les formules non lisses ne sont pas régularisées automatiquement; la dérivée
  actuelle de `abs` peut produire `NaN` en zéro;
- les gros patchs expérimentaux doivent être réduits en petits cas validables
  avant d'être considérés comme des exemples de référence.

La bonne façon de concevoir un patch différentiable dans `faust-rs` est de
partir d'un modèle audio clair, d'une perte scalaire claire, d'un petit nombre
de paramètres, puis d'ajouter progressivement bornes, lissage et affichage.

## 13. Les deux exemples du papier AES 2025

« Faust Autodiff: Towards Audio Domain-Specific Machine Learning »
(T. Rushton, AES AIMLA 2025) différencie les programmes Faust au niveau de
la source : signaux duaux `<s, grad s>` et une règle par opérateur de
composition, implémentées par pattern matching dans une bibliothèque Faust.
Ses deux exemples sont dans le corpus avec le routage du papier, les
paramètres comme graines curseurs :

- [`fad_neuron_sigmoid.dsp`](../tests/corpus/fad_neuron_sigmoid.dsp) et
  [`rad_neuron_sigmoid.dsp`](../tests/corpus/rad_neuron_sigmoid.dsp), le
  neurone `y = sigmoid(w . x + b)` de son listing 4, avec `ro.interleave`
  (un `route`) dans le produit scalaire ;
- [`fad_iir_transposed.dsp`](../tests/corpus/fad_iir_transposed.dsp) et
  [`rad_iir_transposed.dsp`](../tests/corpus/rad_iir_transposed.dsp), l'IIR
  d'ordre 2 de son listing 5, sa soustraction re-routée avec `_` et `!`.

Les limites du papier ne s'appliquent pas ici, parce que `fad` et `rad`
travaillent sur le graphe de signaux après propagation : `route`, le
`ma.sub` non appliqué de `fi.iir` et les widgets ont disparu.
`fad(fi.iir(bv, av), seeds)` donne les tangentes de la fixture au bit près.
Les tests sont dans
[crates/compiler/tests/aes_autodiff_paper.rs](../crates/compiler/tests/aes_autodiff_paper.rs) :
forme fermée pour le neurone, différences finies à chaque trame pour l'IIR
sous `fad`, totaux de bloc pour l'IIR sous `rad`.

## 14. Quelle taille de FIR ou d'IIR compile

Mesuré le 2026-09-23 avec `faustprobe --double --block 256 -n 48000 --time`
(Cranelift, un portable Apple), les coefficients comme graines curseurs, le
faisceau complet de lanes primal et dérivées en sorties. Les gradients ont
été vérifiés par différences finies sur des lanes prises au hasard jusqu'à
l'ordre 256 (`--fd-check` avec un `--grad-lane` explicite, `--train`
attribuant les lanes dans l'ordre de sa liste).

FIR de N taps (`fi.fir`, N graines) :

| N | fad compile | fad calcul | rad compile | rad calcul | mémoire |
|---|---|---|---|---|---|
| 64 | 24 ms | 428× temps réel | 30 ms | 61× | 25 Mo |
| 256 | 0,2 s | 37× | 0,2 s | 9,6× | 70 Mo |
| 1024 | 5,6 s | 6× | 5,4 s | 1,4× | 0,6 à 0,7 Go |
| 4096 | 281 s | 1,1× | 265 s | 0,16× | 5 à 9 Go |

IIR forme directe d'ordre N (`fi.iir`, 2N + 1 graines) :

| N | fad compile | fad calcul | rad compile | rad calcul |
|---|---|---|---|---|
| 16 | 69 ms | 103× | 19 ms | 131× |
| 64 | 2,5 s | 1,4× | 0,13 s | 16× |
| 256 | 198 s | 0,05× | 0,8 s | 4,3× |

Cascade de K biquads (`fi.tf2`, 5K graines) :

| K | fad compile | fad calcul | rad compile | rad calcul |
|---|---|---|---|---|
| 16 | 1,05 s | 3,5× | 42 ms | 64× |
| 64 | 189 s | 0,11× | 0,21 s | 10× |
| 256 | abandon après 900 s | | 2,6 s | 2,1× |

Ce qu'on peut en dire :

- **FIR : quelques centaines de taps confortablement, un millier au prix
  de 5 s de compilation, 4096 est la limite pratique** (des minutes et des
  gigaoctets, encore temps réel en `fad`). Le coût est quadratique : chaque
  lane de dérivée est elle-même un FIR de N taps, le graphe a N² nœuds.
  Au-delà, il faudrait un FIR comme tableau de coefficients avec une boucle,
  ce que ni Faust ni les règles d'AD n'ont aujourd'hui.
- **IIR : en `rad`, l'ordre 256 ou 256 sections compilent en moins de 3 s
  et tournent en temps réel.** La passe inverse par blocs est linéaire en la
  taille du corps. En `fad` sur une récursion, chaque tangente est une
  récursion complète : quadratique, l'ordre 64 ou 64 sections est le plafond
  raisonnable (2 à 3 minutes, 5 Go), 256 ne compile pas.
- **Règle simple** : `fad` pour les petits graphes et l'apprentissage dans
  le graphe, `rad` dès qu'il y a une récursion d'ordre élevé ou plus d'une
  centaine de paramètres.

Face à une bibliothèque en échantillonnage fréquentiel comme FLAMO (Dal
Santo et al., 2025) : là, un FIR de 4096 taps est un produit de réponses en
fréquence, gratuit en gradient, et un FDN 6×6 se différencie par une
inversion de matrice ; les tailles ci-dessus sont celles du domaine
temporel, échantillon par échantillon, avec la récursion exacte. Leurs cas
(FDN à 6 lignes avec matrice orthogonale, un GEQ par ligne, FIR d'acoustique
active de quelques milliers de taps) sont atteignables en `rad` sauf le
dernier, où des milliers de taps par canal sur plusieurs canaux dépassent ce
qui compile. Deux différences de fond : le repliement temporel qu'ils
doivent atténuer n'existe pas ici, et leur perte spectrale reste à écrire
chez nous. La lane d'un tap ou d'un pôle situé au-delà du bloc est nulle en
`rad` (h1023 avec un bloc de 256) : l'horizon documenté.

### Face à la littérature DDSP

Les IIR différentiables en domaine temporel de la littérature restent aux
ordres 1 à 6. Kuznetsov, Parker et Esqueda (DAFx 2020) entraînent des
sections d'ordre 1 et 2, un espace d'états d'ordre 2 et 6 et trois biquads
en série, par rétropropagation tronquée sur des séquences de 2048
échantillons avec l'autograd de PyTorch. Yu et al. (DAFx 2024, torchlpc)
écrivent la passe arrière d'un filtre tout-pôle comme un filtrage
inverse : ordres 1 (compresseur), 2 (TB-303), 6 (phaser) ; un pas
d'optimisation sur le TB-303 prend 29 à 32 ms en temporel contre 57 à
1795 ms en échantillonnage fréquentiel selon la fenêtre (lot de 34 notes,
M1 Pro), un entraînement 17 minutes contre 43. Yu et Fazekas (arXiv
2511.14390, philtorch) donnent la forme espace d'états générale avec
gradients analytiques en noyau C++/CUDA, mesurée à l'ordre 2 seulement,
« les ordres supérieurs étant des cascades de sections » : sur un
i7-7700K mono-thread, 2^20 échantillons (65 s à 16 kHz) prennent environ
10 ms en avant et autant en arrière, de l'ordre de 6000× temps réel par
passe ; l'autograd naïf est « au moins 1000× » plus lent, l'échantillonnage
fréquentiel entre les deux.

Nos mesures sur le même genre d'objet, 4 biquads en `rad` à 576× temps
réel avec le primal et ses 20 lanes de gradient en une passe, un IIR
d'ordre 4 à 1585×, sont dans la classe de ces noyaux dédiés (un facteur de
quelques unités, tout le faisceau émis d'un coup) et trois ordres de
grandeur au-dessus de la pratique DDSP par autograd. La différence
structurelle : un noyau par forme de filtre là-bas, un compilateur pour
n'importe quel corps ici. Personne n'entraîne en temporel un IIR d'ordre
64 ou 256 en forme directe ; les ordres élevés passent par
l'échantillonnage fréquentiel (Nercessian 2020 pour les cascades de
biquads d'égaliseur, FLAMO, les FDN d'Aalto), là où `rad`, linéaire en le
corps, les compile en moins de 3 s.

Les FIR de la littérature ne passent jamais par le temporel au-delà de
quelques dizaines de taps. DDSP (Engel 2020) filtre par échantillonnage
fréquentiel, 65 magnitudes par trame, fenêtre de Hann de 257, hop 256, et
convolue des réponses de réverbération de 10 000 à 100 000 échantillons par
FFT, la convolution directe étant « intraitable ». L'acoustique active
différentiable (De Bortoli, DAFx 2024) apprend des matrices de 2×2 à 13×4
FIR d'ordre 100 et 1000 échantillonnées sur 480 000 points de fréquence,
lots de 2400 points, 10 époques. GRAFX (Lee, DAFx 2024) a un égaliseur FIR
à phase nulle de 2047 taps par IFFT de 1024 log-magnitudes, convolution
FFT, et rend des graphes de 350 à 400 processeurs à 25 à 100 graphes par
seconde sur RTX 3090 avec 5 sources de 2^17 échantillons. Le FDN colorless
(Dal Santo, DAFx 2023) a 4, 6 ou 8 lignes, 6000 à 9000 modes, sur 480 000
points de fréquence ; RIR2FDN (2024), 6 lignes, 5,7 à 41,9 s par itération
sur V100 pour environ 1000 itérations. Notre limite de 4096 taps par
expansion du graphe temporel couvre un filtre de l'acoustique active
(ordre 1000) mais pas sa matrice complète (52 filtres de 1000 taps), et
reste loin des réponses de réverbération DDSP, différentiables seulement
parce que la FFT rend le gradient gratuit : un FIR long a besoin d'une
représentation tableau plus boucle, ou d'un chemin FFT, avant d'être
compétitif.

| | littérature temporelle | littérature fréquentielle | faust-rs |
|---|---|---|---|
| IIR petit ordre | notre classe de vitesse, noyaux dédiés | plus lent, repliement | compilé, exact |
| IIR ordre élevé | absent | seule voie, avec repliement | `rad` linéaire, ordre 256 |
| FIR long | absent | FFT, gratuit | 4096 taps au plus |
| lot, GPU | oui | oui | non mesuré |
| perte spectrale | rare | native | à écrire |

La littérature confirme deux choses : le temporel exact bat le fréquentiel
dès qu'il est compilé (la conclusion de Yu et Fazekas est la nôtre), et le
fréquentiel garde le FIR long et le grand FDN. Nos faiblesses ne sont pas
la vitesse par échantillon mais l'absence de lot et de perte spectrale, et
le FIR long.

Sources : [Kuznetsov et al. 2020](https://www.dafx.de/paper-archive/2020/proceedings/papers/DAFx2020_paper_52.pdf),
[Yu et al. 2024](https://arxiv.org/abs/2404.07970),
[Yu et Fazekas 2025](https://arxiv.org/abs/2511.14390),
[Engel et al. 2020](https://arxiv.org/abs/2001.04643),
[De Bortoli et al. 2024](https://www.dafx.de/paper-archive/2024/papers/DAFx24_paper_64.pdf),
[Lee et al. 2024](https://arxiv.org/abs/2408.03204),
[Dal Santo et al. 2023](https://www.dafx.de/paper-archive/2023/DAFx23_paper_32.pdf),
[Dal Santo et al. 2024](https://arxiv.org/abs/2404.00082),
[FLAMO](https://arxiv.org/abs/2409.08723).

## Voir aussi

- [fad-note-en.md](fad-note-en.md) — surface et implémentation de FAD.
- [rad-usage-en.md](rad-usage-en.md) — workflows RAD pilotés par l'hôte.
- [rad-note-en.md](rad-note-en.md) — algorithme RAD et table des règles.
- [ondemand-note-fr.md](ondemand-note-fr.md) — les primitives de domaine d'horloge.
- [optimizers-overview-fr.md](../libraries/optimizers-overview-fr.md) et
  [optimizers-ddsp-tutorial-fr.md](../libraries/optimizers-ddsp-tutorial-fr.md)
  — la bibliothèque d'optimiseurs expliquée aux débutants, et un tutoriel pas
  à pas.
