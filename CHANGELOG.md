# Changelog

## 0.2.1 — 2026-11-13

- Added Aurora Viewer support: Shelfmark finds Aurora's encrypted inventory cache, reads Aurora's cache key from the system credential store (read-only), and imports the snapshot like a Firestorm one. Live actions remain Firestorm-only. See [Aurora notes](docs/AURORA.md).
- Fixed binary LLSD dates being read with the wrong byte order.
- Fixed a custom Firestorm cache location in settings.xml not being found.
- Fixed imports aborting on a repeated folder or item id, and folder links failing to match on upper-case UUIDs.
- Fixed "database is locked" errors when saving tags during other writes.
- Search now matches multi-word queries across name, folder and tags, and highlights matching folders.
- Menus close on outside click; arrow keys move through result lists; dialogs trap and restore focus.
- Improved text contrast, empty states and labels for screen readers.
- Refreshed the project page with a dark theme, larger text and an FAQ.

## 0.2.0 — 2026-10-21

- Added an optional Firestorm Mini viewer for confirmed, single-item Trash moves on Windows.
- Updated the patch for Firestorm 7.2.5 and added a workflow that builds and publishes the Windows installer.
- Kept Shelfmark’s personal-use terms separate from Firestorm’s LGPL-licensed viewer and patch.
- Expanded the install guide, privacy notes, and project page with clear platform support and current limits.


## 0.1.1 — 2026-10-15

- Clarified which features are available on Windows, macOS, and Linux.
- Expanded the user guide and added a FAQ for cache imports, review tools, privacy, and Firestorm actions.
- Updated the current project terms to personal, non-commercial use.
- Refined the in-app guide and project page so the Windows-only Trash integration is clear.

This release changes documentation and in-app guidance; it does not change inventory handling.

The earlier 0.1.0 release was published under MIT terms. Removing that release from the official repository cannot revoke rights for copies already obtained. This release and later releases use the personal-use terms in [LICENSE](LICENSE).
