# Tests et validation

## Vérifications automatiques

Depuis la racine du dépôt :

```sh
cargo fmt --check
cargo test --locked
cargo test --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo check --locked --no-default-features
cargo check --locked --no-default-features --features x11
cargo check --locked --no-default-features --features wayland
cargo check --locked --no-default-features --features gpu
cargo check --locked --no-default-features --features gpu,x11
cargo check --locked --no-default-features --features gpu,wayland
```

La suite couvre notamment les limites d'allocation, l'alpha prémultiplié, l'arrondi
de l'échelle, les frames étrangères ou obsolètes, les files bornées, l'ordre des
notifications Present et le nettoyage du thread. Le type public est vérifié
`Send + Sync` à la compilation.

Les suites par défaut et avec toutes les features ont chacune passé 15 tests,
documentation comprise, avec Rust 1.97.1. Rust 1.85 est le minimum déclaré ; son
exécution reste à ajouter à la matrice de validation. Les tests graphiques sont
ignorés par défaut et exigent une session compatible.

## Tests graphiques

```sh
cargo test --features gpu --test gpu_runtime -- --ignored --nocapture --test-threads=1
cargo test --test x11_protocol -- --ignored --nocapture
cargo test --features gpu --test visual_input manual_wayland_visual_clicks -- --ignored --nocapture --exact
```

- `gpu_runtime` vérifie la fermeture, les ressources qui survivent à la fenêtre,
  le refus des commandes synchrones dans un callback et l'indépendance de deux
  fenêtres GPU utilisant la même instance Vulkan.
- `x11_protocol` relit les pixels de sa propre fenêtre, vérifie les régions
  d'entrée, le focus et la destruction de la fenêtre.
- `manual_wayland_visual_clicks` affiche un damier et deux bandes colorées.
  Trois clics réels doivent atteindre successivement le fond, l'overlay, puis
  le fond. La bande verte est semi-transparente ; la bande rouge est opaque.

`visual_input` contient aussi des essais de capture via l'interface KWin et de
clics XTEST. Les captures dépendent des permissions normales du compositeur.
Le test XTEST contrôle la fenêtre sous le pointeur avant d'envoyer un bouton et
demande sa restauration à la fin. Exécuter ces essais individuellement dans une
session de test, avec les noms listés par `cargo test --test visual_input -- --list`.

## Stabilité GPU

```sh
cargo run --features gpu --example gpu_stability -- cycles 100 /tmp/liboverlay-cycles.tsv
cargo run --features gpu --example gpu_stability -- soak 300 /tmp/liboverlay-soak.tsv
```

Le programme exige un adaptateur matériel et affiche une petite surface passive.
Il relève le RSS, le nombre de descripteurs et le nombre de threads du processus.
Les cycles alternent fermeture explicite et destruction du dernier handle, avec
des ressources applicatives GPU conservées jusqu'après la fermeture. Le rendu
prolongé encode des commandes WGSL et attend la fin du travail soumis à la queue.

Le mode diagnostique `instances` crée volontairement des instances distinctes,
sans passer par le cache de la bibliothèque. Il sert à isoler le coût ou les
ressources conservées lors de la découverte des pilotes.

## Résultats graphiques de référence

Essais réalisés le 29 octobre 2026 sur KDE/KWin Wayland, avec une NVIDIA GeForce
RTX 3070 Ti Laptop GPU, pilote 580.178.04, à une échelle de 125 % :

| Essai | Résultat |
|---|---|
| 100 créations/fermetures GPU | Succès, 300 frames soumises ; 30 descripteurs et 3 threads après chaque fermeture |
| RSS sur les cycles | Plateau après initialisation : maxima de 155 668 Kio aux cycles 10–19 et de 155 828 Kio sur les dix derniers |
| Rendu continu pendant 300 s | 18 504 frames soumises, aucun arrêt ni intervalle de cinq secondes sans soumission |
| RSS pendant le rendu, après dix secondes | Entre 146 692 et 146 696 Kio |
| Deux fenêtres GPU simultanées | Fermeture de la première, puis nouveau dessin sur la seconde : succès |
| Clics réels | Pression et relâchement reçus uniquement par la cible attendue aux trois étapes |
| Aspect composé | Damier visible à travers la bande verte et les marges ; rouge opaque ; apparence confirmée pendant le contrôle manuel |

La répétition d'instances Vulkan distinctes avait révélé une accumulation de
descripteurs dans cette configuration. Le partage d'une instance par processus
évite cette croissance dans les essais réalisés. Ce cache garde volontairement
des ressources natives jusqu'à la fin du processus.

Sur la session NVIDIA/XWayland testée, Vulkan annonce uniquement un mode alpha
opaque. Le GPU obligatoire avec transparence imposée échoue explicitement ; le
GPU préféré utilise alors le CPU et conserve la transparence. L'opacité doit être
autorisée explicitement pour utiliser cette surface GPU X11.

Les captures automatiques du bureau composé n'étaient pas autorisées par KWin.
La validation visuelle repose sur une observation manuelle ; les événements de
clics et les pixels de la fenêtre X11 ont été vérifiés par les tests.

## Chemins d'erreur et musl

```sh
cargo build --example diagnose
python3 scripts/check_failure_paths.py
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl --examples
```

Le script utilise une socket Unix temporaire pour un serveur Wayland muet.
L'absence d'affichage, un numéro X11 hors plage et ce serveur muet ont produit des
erreurs typées, sans panic ; le dernier scénario expire après environ cinq secondes.

Les exemples CPU musl ont été exécutés sur la session de référence, avec échelle
fractionnaire Wayland, Present XWayland et repli automatique de backend. Leur
inspection ELF ne montrait ni entrée `DT_NEEDED` ni interpréteur dynamique.
La voie GPU testée utilise la cible GNU et des bibliothèques natives chargées
dynamiquement.

## Limites de couverture

Ces résultats portent sur une charge légère pendant cinq minutes et 100 cycles.
Ils ne mesurent pas la VRAM, la cadence physique d'affichage ou sa latence.
Ils ne couvrent pas encore les longues durées, la perte du périphérique, le
redémarrage du compositeur, le branchement d'écrans, les autres pilotes, Xorg
natif ou l'ensemble des modes plein écran. La [feuille de route](../ROADMAP.md)
conserve ces vérifications ouvertes.
