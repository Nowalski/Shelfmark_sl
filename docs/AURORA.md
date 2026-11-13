# Aurora Viewer notes

Shelfmark can import inventory snapshots saved by [Aurora Viewer](https://github.com/Aurora-Viewer/Aurora-Viewer), a Second Life viewer written in Rust. Search, folders, tags, notes, collections and candidate groups work the same as for a Firestorm snapshot.

## How to import

1. Log in with Aurora on this computer and let your inventory load, then quit Aurora so it saves its cache.
2. In Shelfmark, choose **Import inventory cache**. Aurora snapshots are labelled **Aurora** in the list.
3. Select the avatar and confirm.

Shelfmark looks in Aurora's default cache folder, the folder recorded from Aurora's last run (`cache_location.txt`) and a custom folder chosen in Aurora's preferences (`cache_location` in `settings.json`, used as `<folder>/AuroraViewerCache`):

| Platform | Default cache folder | Settings folder |
| --- | --- | --- |
| Windows | `%LOCALAPPDATA%\Aurora\AuroraViewer\cache` | `%APPDATA%\Aurora\AuroraViewer\config` |
| macOS | `~/Library/Caches/org.Aurora.AuroraViewer` | `~/Library/Application Support/org.Aurora.AuroraViewer` |
| Linux | `~/.cache/auroraviewer` | `~/.config/auroraviewer` |

The inventory snapshot itself is in `store/inventory/<name>.inv` inside the cache folder. You can also choose that file with **Browse**.

## Why Shelfmark needs Aurora's cache key

Aurora encrypts its disk cache (AES-256-GCM) with a random key created for each installation and kept in the system credential store: Windows Credential Manager, macOS Keychain or the Secret Service on Linux, under *Aurora Viewer cache / disk cache key v1*. When no credential store is available, Aurora keeps the key in `cache.key` in its settings folder. File names are keyed hashes of the avatar UUID, so Shelfmark matches each file to an avatar using the account folders in Aurora's settings (`config/accounts/<uuid>`).

Shelfmark **only reads** that key, on the same computer and user account as Aurora. It never creates, changes or deletes it, never writes into Aurora's folders, and never sends the key or your inventory anywhere. On macOS the Keychain may ask once whether Shelfmark may read the item.

If the key cannot be read (for example on Linux without a running Secret Service, or after Aurora reset its cache), Shelfmark shows a notice in the cache chooser and leaves the files alone.

## Limits

- Aurora snapshots are catalogued under their own label (`<avatar>@aurora`), separate from Firestorm imports of the same avatar, because Aurora's cache does not record which grid it came from.
- Aurora has no LEAP interface, so live viewer connection and Trash moves are not available for Aurora. Use Aurora itself to move or delete items.
- Item thumbnails are not shown for Aurora snapshots: Aurora's texture cache uses a different, encrypted format.
- Aurora saves only folders it has fetched. Open your inventory in Aurora and let it finish loading for a complete snapshot.
- Shelfmark reads Aurora inventory cache format version 3. Aurora is under active development; if a future version changes the format, Shelfmark reports the version it found and changes nothing.
