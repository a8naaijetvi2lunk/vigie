# Sécurité

## Signaler une vulnérabilité

Merci de **ne pas ouvrir d’issue publique** pour un problème de sécurité.
Utilisez plutôt le signalement privé de GitHub : onglet **Security** du dépôt,
puis **Report a vulnerability**.

Indiquez si possible la version concernée, les étapes pour reproduire et l’impact estimé.

## Périmètre

Vigie manipule un secret : le jeton OAuth que Claude Code stocke dans
`~/.claude/.credentials.json`. Sont notamment dans le périmètre :

- toute fuite de ce jeton (journal, cache, historique, interface, requête vers un autre hôte que `api.anthropic.com`) ;
- toute conservation, tout affichage ou toute transmission de contenu de conversation ;
- toute exécution de code ou injection via les fichiers de session lus par Vigie ;
- toute écriture en dehors des dossiers de données de Vigie et de la clé de démarrage automatique.

## Principes appliqués

- Le jeton est relu sur disque à chaque cycle, libéré dès la réponse, jamais écrit ni journalisé.
- Vigie ne rafraîchit jamais le jeton : seul Claude Code le fait.
- Aucune télémétrie, aucune dépendance réseau hormis l’endpoint d’usage d’Anthropic.
- Les fenêtres n’ont que les permissions Tauri `core:default` et `core:window:allow-start-dragging`,
  en plus des commandes IPC de Vigie, qui n’acceptent aucun chemin de fichier.
