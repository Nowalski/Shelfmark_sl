# Firestorm compatibility

Shelfmark imports the inventory cache written by Firestorm. It reads the selected cache to create a local catalogue and does not write to that cache or change inventory in the background.

## Cache discovery

Shelfmark checks the standard Firestorm cache location for your platform and, on Windows, the custom `CacheLocation` saved in Firestorm settings. Choose **Browse** to select another folder.

During discovery, Shelfmark lists matching cache files and reads file names, modification dates, and optional cached avatar names. It parses the selected inventory file only after you confirm the import. The cache itself remains read-only.

Close Firestorm before importing or refreshing if you want its latest saved changes. When the viewer is running, those changes may not have reached the cache file yet.

## Live viewer connection

Live viewer control is currently available on Windows only. From Home, choose **Stock Firestorm** to connect the viewer you already use. Close all Firestorm windows first: the LEAP option is read only at startup.

When the connection is ready, Shelfmark reads the active avatar UUID and can select the matching imported snapshot. LEAP does not identify the grid, so confirm Firestorm is on the Second Life main grid. Packaged releases use stock LEAP for connection and avatar checks; stock LEAP inventory moves remain development-only until the Delete route has repeated live validation.

## Optional Firestorm Mini bridge

The experimental Firestorm Mini build uses a small viewer-source patch instead of asking the inventory tree to process a Delete key. It sends the exact item and source folder to Firestorm, which applies its normal move path and asks you to confirm. Shelfmark then checks the live item location through LEAP.

The patch now targets the Firestorm 7.2.5 release. Download the optional [Firestorm Mini installer](https://github.com/Nowalski/Shelfmark_sl/releases/tag/firestorm-mini-7.2.5.81788), or build from source using the [patch instructions](../integrations/firestorm-mini/README.md). The prebuilt installer uses Firestorm’s open configuration and does not include FMOD sound or voice. For a full audio build, compile the patched source using Firestorm’s build guide. Firestorm Mini uses its own viewer channel and can be installed alongside stock Firestorm. Close other Firestorm windows before starting the Shelfmark connection.

The patch is optional and experimental. It only moves one item at a time to Trash on the Second Life main grid. It does not enable ordinary-folder moves, batch moves, or permanent deletion. The source patch is under LGPL-2.1-only; the rest of Shelfmark keeps the repository’s personal-use terms.

## Move an item to Trash

To send an item to Trash:

1. Select the matching avatar snapshot in Shelfmark.
2. In Firestorm, keep the **All Items** inventory tab open and clear its inventory search field.
3. Select one item in Shelfmark and choose **Move to Trash**.
4. Firestorm Mini asks you to confirm the exact item. Shelfmark checks the item’s live parent folder before reporting success. In stock Firestorm development builds, Shelfmark uses the viewer’s normal Delete action instead.
5. Close Firestorm and refresh the snapshot after the viewer has saved its cache.

Trash actions require a matching live Firestorm session on the Second Life main grid. Shelfmark does not edit the cache file. The Keep safe control is a local review reminder; it does not change inventory or prevent a live action.

Moves to ordinary folders remain paused. Stock LEAP does not reliably confirm which item is on the inventory clipboard after Cut, so Shelfmark cannot safely rule out pasting an older clipboard item. Make those moves in Firestorm.

The macOS and Linux builds can import and browse local snapshots, but do not support the Firestorm Mini bridge or live Trash actions. Stock LEAP inventory changes remain disabled in packaged release builds until the Delete-based route completes repeated live validation.

Firestorm and Second Life are named only to describe compatible software and files. Shelfmark is an independent project and does not include Firestorm code or assets.
