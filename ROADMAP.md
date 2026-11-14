# Travaux restants

Le socle 0.2 comprend les overlays Wayland layer-shell et X11/XWayland, le passage
des clics configurable, le rendu CPU, Vulkan optionnel, Present pour X11 et
l'échelle fractionnaire Wayland. Les cases ci-dessous concernent les travaux
encore ouverts. La [validation](docs/TESTING.md) décrit les essais déjà réalisés.

## Priorité 1 : robustesse et couverture

- [ ] Étendre le test GPU à plusieurs heures et à des surfaces plus grandes.
- [ ] Mesurer séparément la VRAM et vérifier les ressources après de nombreux cycles.
- [ ] Tester la perte de connexion pendant le rendu et le redémarrage du compositeur
  dans une session isolée ; vérifier erreur, fermeture et libération des ressources.
- [ ] Tester la perte du périphérique Vulkan et les échecs de configuration de surface.
- [ ] Exercer les appels concurrents sur les handles, notamment fermeture et dessin
  depuis plusieurs threads.
- [ ] Vérifier sur un vrai compositeur les changements rapides d'échelle et de taille,
  ainsi que les fenêtres masquées ou privées de callbacks de rendu.
- [ ] Automatiser compilation, tests, formatage et Clippy sur GitHub Actions.
- [ ] Exécuter la suite avec le compilateur minimum déclaré, Rust 1.85.

## Priorité 2 : multiécran et compatibilité

- [ ] Repositionner l'overlay X11 au branchement, retrait ou déplacement d'un écran.
- [ ] Vérifier les dispositions à coordonnées négatives et les écrans à échelles mixtes.
- [ ] Définir et tester le comportement lorsqu'une sortie sélectionnée disparaît.
- [ ] Valider Xorg natif, un serveur sans Present et un affichage sans composition.
- [ ] Élargir les essais à d'autres compositeurs et pilotes Intel, AMD et NVIDIA.
- [ ] Tester l'absence de layer-shell, fractional-scale ou viewporter et les replis associés.
- [ ] Vérifier les pilotes Vulkan absents, incompatibles ou logiciels, en respectant
  les exigences de transparence et le choix explicite d'autoriser le logiciel.
- [ ] Vérifier l'empilement et le focus avec des applications en plein écran.
- [ ] Exécuter les binaires CPU musl sur plusieurs distributions.

## Priorité 3 : performances

- [ ] Ajouter MIT-SHM pour le transfert CPU X11 ; conserver `PutImage` comme repli.
- [ ] Mesurer CPU, RAM, VRAM, latence et cadence d'affichage pour les deux renderers.
- [ ] Évaluer la réutilisation des allocations de frames CPU et les mises à jour partielles.
- [ ] Comparer le coût des différentes dimensions et fréquences d'actualisation.

## Extensions possibles

- [ ] Exposer les axes de défilement de la souris.
- [ ] Ajouter une traduction clavier et une saisie de texte composée adaptées aux usages.
- [ ] Étudier le tactile et les régions d'entrée personnalisées.
- [ ] Ajouter des exemples intégrant un moteur de texte ou de widgets.

## Préparation d'une version distribuée

- [ ] Stabiliser les contrats publics et documenter les changements incompatibles.
- [ ] Ajouter un journal des versions et des tags après validation.
- [ ] Préparer la publication sur crates.io et vérifier le contenu du paquet final.

La compatibilité reste déterminée par les capacités négociées avec le serveur
graphique et le pilote. L'affichage au-dessus de toute application plein écran
ou d'un écran de verrouillage ne fait pas partie des garanties actuelles.
