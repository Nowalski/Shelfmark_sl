# Firestorm Mini bridge (experimental)

This folder contains a small patch for the Firestorm 7.2.5 source tree. It adds a confirmed, single-item move-to-Trash command that Shelfmark can send to a running patched viewer. Shelfmark then reads the live item location through LEAP before it reports success. “Firestorm Mini” is Shelfmark’s informal name for this optional build; it is not a separate Firestorm edition.

The optional [Windows installer](https://github.com/Nowalski/Shelfmark_sl/releases/tag/firestorm-mini-7.2.5.81788) is built from Firestorm’s open configuration, which omits FMOD sound and voice. It uses its own viewer channel and can be installed alongside stock Firestorm, though Shelfmark connects to only one running viewer at a time. For a full audio build, compile the source yourself with the dependencies and permissions required by Firestorm. The installer and source patch are experimental and are not maintained or endorsed by the Firestorm team.

## Apply the patch

Use a clean checkout of Firestorm’s 7.2.5 release branch. The patch was prepared against official commit [`4ac201f`](https://github.com/FirestormViewer/phoenix-firestorm/commit/4ac201fdc56127de27de34567286885f4e75d194), tagged [`Firestorm_Release_7.2.5.81788`](https://github.com/FirestormViewer/phoenix-firestorm/tree/Firestorm_Release_7.2.5.81788). From the viewer repository root, run:

```sh
git apply /path/to/Shelfmark_sl/integrations/firestorm-mini/patches/firestorm-7.2.5-shelfmark-native-trash.patch
```

The patch applies to that source revision. The public workflow builds an installer from the open configuration and checks that the patch applies before compiling. That configuration has no FMOD sound or voice. Build the viewer using Firestorm’s Windows build guide if you need those features. The patched viewer uses a separate Windows window class so a stock viewer will not receive Shelfmark URLs. Close other Firestorm processes before starting a Shelfmark LEAP session.

In Shelfmark, open **Home**, choose **Firestorm Mini**, select the patched `Firestorm.exe`, and sign in to the Second Life main grid. Pick the matching imported snapshot. When you choose **Move to Trash**, Firestorm shows a confirmation prompt. Shelfmark waits for the live parent folder to change and remain in Trash before updating the catalogue. After closing the viewer, browse to its cache folder if Refresh does not find the updated snapshot.

## Scope and limits

- Windows and the Second Life main grid only.
- One inventory item at a time, moved to Trash. Folder moves and batch operations are not included.
- The viewer checks the active avatar, source folder, grid, inventory availability, and Firestorm’s normal removable-item rules. The viewer asks for confirmation on every move.
- A rejected, delayed, or unrecognized request is not reported as successful. Check the item in Firestorm if Shelfmark times out.
- Firestorm inventory changes are live. Items moved to Trash can be restored in Firestorm; this bridge does not empty Trash or permanently delete items.
- Build and test this patch before using it with important inventory. It has not been validated against every Firestorm configuration.

## License

The Firestorm patch is separately licensed under **LGPL-2.1-only**. See this folder’s [LICENSE](LICENSE) and the [official Firestorm 7.2.5 source license](https://github.com/FirestormViewer/phoenix-firestorm/blob/Firestorm_7.2.5/LICENSE). The rest of Shelfmark remains under the terms in the repository root [LICENSE](../../LICENSE).
