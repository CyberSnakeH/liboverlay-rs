# Utilisation

Les exemples supposent une session graphique Linux locale. Le package Cargo
est `liboverlay-rs` et le module importé est `liboverlay`.

## Dépendance

```toml
[dependencies]
liboverlay-rs = { git = "https://github.com/CyberSnakeH/liboverlay-rs", branch = "main" }
```

Pour Vulkan, ajouter `features = ["gpu"]`. Les features `wayland` et `x11` sont
activées par défaut ; `default-features = false` permet de choisir les backends.

## Créer et fermer une fenêtre

```rust,no_run
use liboverlay::{Anchor, OverlayBuilder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let overlay = OverlayBuilder::new()
        .with_title("Monitoring")
        .with_size(360, 120)
        .with_anchor(Anchor::TopRight)
        .with_margin(16)
        .with_click_through(true)
        .build()?;

    println!("{:#?}", overlay.backend_report());
    overlay.close()?;
    Ok(())
}
```

Une application conserve le handle pendant sa boucle de rendu. Le handle peut
être cloné et partagé entre threads. `close()` ferme la fenêtre pour tous les
handles et attend le nettoyage du thread d'affichage.

## Dessin CPU

`draw()` reçoit un tableau de pixels et les dimensions physiques. Les pixels
utilisent `0xAARRGGBB` avec alpha prémultiplié : `premultiply()` réalise la conversion.
Les pixels à zéro sont transparents.

```rust,ignore
overlay.draw(|pixels, width, height| {
    pixels.fill(0);
    for y in 12..height.min(80) {
        for x in 12..width.min(240) {
            pixels[(y * width + x) as usize] = liboverlay::premultiply(30, 200, 100, 160);
        }
    }
})?;
```

Pour contrôler l'allocation et l'envoi séparément, utiliser `frame()` puis
`present(frame)`. Une soumission réussie signifie mise en file. `FrameSubmitted`
signale l'envoi au serveur graphique, sans confirmer l'affichage physique.

La file peut être pleine et la géométrie peut changer entre allocation et envoi.
Une boucle de rendu traite `QueueFull` et `OutdatedFrame`, récupère les informations
actuelles et redessine. Le [HUD CPU](../examples/basic_hud.rs) montre ce fonctionnement.

## Dessin GPU

Compiler avec `gpu` puis choisir `RendererPreference::GpuRequired` ou
`GpuPreferred`. Le premier impose Vulkan ; le second peut revenir au CPU et
enregistre la raison dans le rapport. Vérifier le renderer effectivement choisi
avant d'appeler les méthodes propres au CPU ou au GPU.

`gpu_context()` fournit le périphérique, la queue, le format et le mode alpha.
Créer les shaders et pipelines avant de dessiner. `draw_gpu()` reçoit un callback
qui encode les commandes d'une frame à l'aide de `GpuFrame`. Ce callback s'exécute
sur le thread privé et doit rester court, sans appeler `close()` ou
`set_click_through()` de façon synchrone.

Le [HUD Vulkan](../examples/gpu_hud.rs) présente un pipeline WGSL complet. Les
couleurs doivent respecter l'alpha prémultiplié. Les adaptateurs logiciels sont
refusés par défaut ; `with_software_gpu(true)` autorise leur utilisation explicite.

## Entrées et géométrie

`set_click_through(true)` laisse passer les clics ; `false` rend la surface
interactive. `poll_events()` retourne les mouvements, boutons, touches brutes,
changements de dimensions et événements de cycle de vie disponibles.

Le clavier n'est pas demandé par défaut. `KeyboardMode::OnDemand` permet une
interaction clavier sur les backends compatibles, sans composition de texte.
Les codes sont exposés selon la convention evdev documentée par l'API.

`info()` retourne les dimensions logiques, les dimensions physiques, le facteur
d'échelle et la génération courante. Les frames CPU utilisent les dimensions
physiques. `ScaleFactor::as_f64()` retourne le facteur usuel et `numerator()`
sa valeur exacte en 120ᵉ.

## Capacités et erreurs

`backend_report()` décrit le backend, le renderer, la transparence, l'entrée,
l'empilement, la cadence, les avertissements et les tentatives de repli.
`WaylandOnly` ou `X11Only` impose un backend. `Requirement::Required` conserve
une exigence forte ; `Preferred` permet les limites documentées.

Les erreurs sont retournées dans `Result`. Une erreur fatale du runtime est
remontée par `poll_events()`. Une perte de connexion ferme l'overlay. Le délai
d'une commande synchrone ne constitue pas une annulation d'une commande déjà
en file ; les callbacks utilisateur doivent donc également éviter les blocages.

## Exemples disponibles

| Exemple | Usage |
|---|---|
| `basic_hud` | Dessin CPU animé et sélection du backend |
| `interactive_toggle` | Alternance entre passage des clics et interaction |
| `diagnose` | Rapport du backend, géométrie et renderer retenu |
| `gpu_hud` | Pipeline WGSL et présentation Vulkan |
| `gpu_stability` | Cycles de création/fermeture et rendu prolongé avec mesures |

Les commandes et les résultats de référence sont regroupés dans
[TESTING.md](TESTING.md).
