# liboverlay-rs

Bibliothèque Rust pour créer des overlays Linux transparents, avec passage des
clics configurable et sélection des capacités au démarrage.

Le package Cargo est `liboverlay-rs` ; le nom utilisé dans le code est
`liboverlay`. La version 0.2 fournit un rendu CPU ARGB prémultiplié, une
présentation Vulkan optionnelle, X11 Present et l'échelle fractionnaire Wayland. Le profil
CPU reste activé par défaut ; la feature `gpu` ajoute la présentation directe.

## Installation

Le dépôt contient la version de développement 0.2.0. Pour l'utiliser directement
dans une application :

```toml
[dependencies]
liboverlay-rs = { git = "https://github.com/CyberSnakeH/liboverlay-rs", branch = "main" }
```

Le nom du module importé reste `liboverlay`. Pour activer Vulkan, ajouter
`features = ["gpu"]` à cette dépendance. Rust 1.85 minimum est déclaré ; consulter
la [couverture de validation](docs/TESTING.md) pour les compilateurs effectivement testés.

Pour lancer les exemples depuis les sources :

```sh
git clone https://github.com/CyberSnakeH/liboverlay-rs.git
cd liboverlay-rs
cargo run --example basic_hud -- auto 10
```

## Documentation

- [Utilisation et API](docs/USAGE.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Tests, résultats et limites de validation](docs/TESTING.md)
- [Travaux restants](ROADMAP.md)

```rust,no_run
use liboverlay::{premultiply, OverlayBuilder, OverlayError};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let overlay = OverlayBuilder::new()
        .with_title("GameHUD")
        .with_size(800, 600)
        .with_click_through(true)
        .build()?;

    println!("{:#?}", overlay.backend_report());
    let start = Instant::now();
    while overlay.is_running() && start.elapsed() < Duration::from_secs(10) {
        overlay.poll_events()?;
        match overlay.draw(|pixels, width, height| {
            pixels.fill(0); // transparent
            for y in 10..height.min(80) {
                for x in 10..width.min(240) {
                    pixels[(y * width + x) as usize] = premultiply(30, 200, 100, 160);
                }
            }
        }) {
            Ok(()) => {},
            Err(OverlayError::QueueFull | OverlayError::OutdatedFrame { .. }) => {},
            Err(error) => return Err(error.into()),
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    overlay.close()?;
    Ok(())
}
```

## Exemples

```sh
cargo run --example basic_hud -- auto 10
cargo run --example basic_hud -- wayland 10
cargo run --example basic_hud -- x11 10
cargo run --example interactive_toggle
cargo run --example diagnose -- auto
cargo run --features gpu --example gpu_hud -- auto 10
cargo run --features gpu --example diagnose -- x11 prefer-gpu
```

`interactive_toggle` alterne toutes les deux secondes entre un rectangle bleu
traversable et un rectangle orange qui reçoit les clics. Le clavier n'est pas
demandé. L'exemple s'arrête après douze secondes. Il ne crée aucun raccourci
global et ne génère aucun clic synthétique.

## Sélection et capacités

`Auto` examine `WAYLAND_DISPLAY`/`WAYLAND_SOCKET`, tente une vraie connexion
Wayland, puis tente `DISPLAY` si nécessaire. XWayland et Xorg utilisent le même
backend X11. Une préférence explicite `WaylandOnly` ou `X11Only` interdit le repli.

| Fonction | Wayland layer-shell | X11 / XWayland |
|---|---|---|
| Transparence | `wl_shm::ARGB8888` | Visual ARGB RENDER et composition |
| Passage des clics | Région d'entrée vide + commit | Région SHAPE INPUT vide, version >= 1.1 |
| Premier plan | Couche layer-shell Overlay | Raise ou demande EWMH, selon le mode |
| Clavier | Aucun par défaut, OnDemand dès layer-shell v4 | Aucun demandé par défaut, fenêtre gérée pour OnDemand |
| Cadence CPU | Callback du compositeur | Present synchronisé, sinon timer |
| Cadence GPU | Swapchain FIFO et callback du compositeur | Swapchain FIFO, si le pilote satisfait les exigences |
| Échelle | Fractionnaire avec fractional-scale + viewporter, sinon entière | Pixels de l'écran X11 |
| Écran nommé | Nom `wl_output`, version >= 4 | Moniteurs RandR, version >= 1.5 |

La transparence est obligatoire par défaut. `Requirement::Preferred` autorise
explicitement une fenêtre opaque sur X11 si la composition ou le visual alpha
manque. `StackingPolicy::RequireOverlayLayer` exige layer-shell ; `PreferOverlay`
autorise les politiques X11 moins fortes. Le rapport conserve les échecs des
tentatives précédentes et les limites du backend choisi. Il décrit les capacités
du protocole, pas une observation des pixels du bureau.

La présence d'un serveur XWayland ne garantit pas l'affichage au-dessus d'une
application Wayland native en plein écran. Les écrans de verrouillage et les
interfaces du compositeur restent sous le contrôle de ce dernier. Sans layer-shell
ni repli X11 satisfaisant les exigences, la création retourne `NoBackend`.

## Rendu, threads et cycle de vie

- Les pixels sont des valeurs numériques `0xAARRGGBB`, avec alpha prémultiplié.
  Utiliser `premultiply(r, g, b, a)` pour convertir des composantes ordinaires.
  L'ordre des octets du serveur X11 est pris en compte pendant le transfert.
- Chaque `frame()` commence avec des pixels transparents. `draw()` est un raccourci
  qui alloue cette frame, appelle la fonction utilisateur puis la présente.
- Un `OverlayWindow` est un handle clonable `Send + Sync`. Le runtime privé garde
  les connexions et objets de protocole. Le dernier handle détruit ferme le runtime.
  `close()` attend le nettoyage des ressources et joint le thread, y compris ses
  destructeurs natifs. Le dernier handle demande également cet arrêt. L'attente
  de la boucle est limitée par le délai de commande ; la jointure finale attend
  aussi la destruction des données locales au thread par les bibliothèques natives.
- `present()` consomme la frame et signifie « mise en file ». `FrameSubmitted`
  signifie « envoyée au serveur », sans confirmer l'affichage physique. Une frame
  remplacée ou devenue obsolète pendant un redimensionnement produit `FrameDiscarded`.
- La file de commandes contient au plus huit entrées et le runtime conserve au
  plus une frame en attente. Il peut abandonner des frames pour garder la plus récente.
  La file d'événements est bornée à 256 entrées ; les mouvements successifs sont
  fusionnés et les événements les plus anciens peuvent être supprimés.
- Wayland garde au plus trois buffers SHM, toutes générations confondues, et
  attend `wl_buffer.release` avant de les réécrire. Une fenêtre masquée peut ne
  plus recevoir de callbacks de dessin ; les commandes d'entrée restent traitées.
- X11 négocie Present 1.0 pour les frames CPU, demande une copie au prochain
  intervalle vertical et attend `CompleteNotify` et `IdleNotify` avant de réécrire
  le pixmap. Une seule présentation est en vol. Sans Present, le timer utilise
  `with_frame_interval()` et le rapport indique `FramePacing::Timer`.
- `set_click_through()` attend l'émission de la requête, indépendamment du rendu.
  Un délai dépassé ne constitue pas une annulation d'une commande déjà en file.
- `poll_events()` retourne les erreurs fatales du runtime. Une perte de connexion
  ferme l'overlay ; aucune migration de backend après affichage n'est implicite.
- Les dimensions des frames sont physiques. `info()` fournit aussi les dimensions
  logiques, l'échelle et la génération. Une frame d'un autre overlay est refusée.
  `WindowInfo.scale_factor` est désormais un `ScaleFactor` exact en 120ᵉ :
  `as_f64()` fournit la valeur usuelle, `numerator()` la valeur du protocole et
  `ScaleFactor::ONE` représente 100 %. Ce changement d'API motive la version 0.2.

## GPU direct

Compiler avec `--features gpu`, puis choisir un `RendererPreference` :

- `Cpu` : comportement par défaut, rendu via `frame()` / `draw()` / `present()`.
- `GpuRequired` : exige une surface Vulkan compatible avec la transparence demandée.
- `GpuPreferred` : essaie Vulkan dans le backend choisi, puis conserve le rendu
  CPU si nécessaire ; la raison du repli figure dans `backend_report().warnings`.

Avec un renderer GPU, `gpu_context()` donne accès au périphérique, à la queue,
au format et au mode alpha. Préparer les shaders et pipelines avant de dessiner.
`draw_gpu()` met en file une fonction qui reçoit une vue de texture, un encodeur
et les dimensions courantes. Cette fonction s'exécute sur le thread d'affichage :
elle doit rester courte et ne pas appeler les commandes synchrones de la fenêtre.
Une fonction de dessin obsolète peut être abandonnée avant son exécution.

Les images de la swapchain sont initialisées transparentes, puis les commandes
sont soumises directement au GPU, sans lecture des pixels vers le CPU. La surface
et ses textures acquises restent privées au runtime. L'exemple
[`gpu_hud.rs`](examples/gpu_hud.rs) prépare un pipeline WGSL puis dessine une barre
animée, avec des marges transparentes. Les couleurs sont prémultipliées dans un
format UNORM non sRGB. Les adaptateurs logiciels sont refusés par défaut ;
`with_software_gpu(true)` permet de les autoriser explicitement pour un diagnostic.

Le mode FIFO est demandé pour la synchronisation verticale. Ce mode et les
comptages `FrameSubmitted` ne prouvent pas la cadence physique de l'écran.
Le GPU n'est retenu que si les modes alpha annoncés satisfont les exigences :
`PreMultiplied`, ou `Inherit` lorsque l'alpha natif de la fenêtre est établi.
Un mode opaque exige `Requirement::Preferred` et est signalé comme tel.

Sur la session NVIDIA/XWayland testée, Vulkan n'annonce que `Opaque`. Un overlay
transparent avec `GpuRequired` est donc refusé ; `GpuPreferred` conserve son alpha
en utilisant le CPU. La présentation GPU transparente et le facteur 125 % ont été
validés dans la session Wayland native. Les autres pilotes peuvent annoncer
d'autres capacités ; aucune liste de bureaux ne remplace cette négociation.

## Échelle fractionnaire

Lorsque `wp_fractional_scale_manager_v1` et `wp_viewporter` sont disponibles,
le backend utilise l'échelle préférée de la surface, arrondit les dimensions
physiques au plus proche avec les demis vers le haut, conserve `buffer_scale = 1`
et fixe la destination du viewport aux dimensions logiques. Un changement
d'échelle renouvelle la génération des frames et produit `Resized` ; les anciens
buffers occupés sont conservés jusqu'à leur libération. Sans cette paire de
protocoles, le backend garde l'échelle entière des sorties.

`KeyboardMode::OnDemand` expose des codes de touches bruts, sans composition de
texte ni traduction XKB. Sur X11 les codes sont normalisés en soustrayant 8, ce qui
suppose la convention habituelle evdev ; les configurations XKB personnalisées
peuvent nécessiter une traduction par l'application. Les axes de défilement,
le tactile et les raccourcis globaux ne sont pas exposés dans cette version.

## Dépendances et portabilité

Sans la feature `gpu`, le socle X11 utilise `x11rb::RustConnection` et Wayland le
backend Rust de `wayland-client`. Il ne nécessite ni en-têtes X11 ni liaison à `libX11`,
`libxcb` ou `libwayland-client`. L'unification des features Cargo par une application
peut toutefois activer le backend système de Wayland : vérifier son propre graphe.

La feature `gpu` utilise le chargeur Vulkan et un pilote ICD. Elle charge
`libwayland-client.so.0` pour Wayland, ou une connexion `libxcb.so.1` supplémentaire
pour la surface Vulkan X11. Ces bibliothèques sont chargées dynamiquement, sans
compilation C manuelle. Dans un binaire compilé avec `gpu`, l'unification des
features active aussi le backend Wayland système pour le mode CPU de ce binaire.
Le profil sans `gpu` conserve le transport Rust et la cible musl statique.

Le profil GPU partage une instance Vulkan initialisée une fois par processus.
Les surfaces et périphériques restent propres à chaque fenêtre. Ce cache évite
de répéter la découverte des pilotes et l'accumulation de ressources constatée
sur le pilote NVIDIA testé. Le cache conserve des ressources natives jusqu'à
la fin du processus ; fermer une fenêtre ne réinitialise pas tout le chargeur GPU.

Les features `wayland` et `x11` sont activées par défaut et peuvent être choisies
séparément, y compris avec `gpu`. Les connexions X11 utilisent uniquement les
sockets Unix locaux. Le transfert SHM pour le rendu CPU X11 et le repositionnement X11
au branchement d'un écran restent des évolutions à implémenter.

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo check --no-default-features --features x11
cargo check --no-default-features --features wayland
cargo test --test x11_protocol -- --ignored --nocapture
cargo test --features gpu --test gpu_runtime -- --ignored --nocapture
cargo run --features gpu --example gpu_stability -- cycles 100 /tmp/liboverlay-cycles.tsv
cargo run --features gpu --example gpu_stability -- soak 300 /tmp/liboverlay-soak.tsv
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl --examples
```

Le profil CPU vise les binaires musl statiques. La voie GPU requiert le chargeur
dynamique et les bibliothèques graphiques mentionnées ci-dessus ; les essais GPU
ont été effectués avec la cible GNU. Aucun profil ne rend disponibles des
protocoles absents du compositeur.

Le test `x11_protocol` est volontairement ignoré par défaut : il ouvre sa propre
petite fenêtre, relit ses pixels et sa région d'entrée côté serveur, contrôle le
focus et vérifie sa destruction. Il requiert un affichage X11 local avec composition.
Il ne déplace pas la souris et n'injecte pas d'événement d'entrée.

Les essais sur la session de référence couvrent notamment 100 créations/fermetures
GPU, cinq minutes de rendu continu et le routage de clics réels entre une fenêtre
de fond et un overlay Wayland. Les résultats et leurs limites sont décrits dans
[la documentation de validation](docs/TESTING.md). Les vérifications encore à
effectuer et les évolutions prévues figurent dans la [feuille de route](ROADMAP.md).

## Références

- [Wayland : surfaces, SHM, régions d'entrée et callbacks](https://wayland.freedesktop.org/docs/html/apa.html)
- [Protocole layer-shell](https://github.com/swaywm/sway/blob/master/protocols/wlr-layer-shell-unstable-v1.xml)
- [Architecture XWayland](https://wayland.freedesktop.org/docs/book/Xwayland.html)
- [SHAPE 1.1](https://www.x.org/releases/X11R7.5/doc/Xext/shape.pdf)
- [EWMH](https://specifications.freedesktop.org/wm/latest-single/)
- [Protocole fractional-scale](https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/staging/fractional-scale/fractional-scale-v1.xml)
