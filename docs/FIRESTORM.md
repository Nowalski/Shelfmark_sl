# Firestorm compatibility

Shelfmark imports the inventory cache written by Firestorm. It reads the selected cache to create a local catalogue and does not write to that cache or change inventory in the background.

## Cache discovery

Shelfmark checks the standard Firestorm cache location for your platform and, on Windows, the custom `CacheLocation` saved in Firestorm settings. Choose **Browse** to select another folder.

During discovery, Shelfmark lists matching cache files and reads file names, modification dates, and optional cached avatar names. It parses the selected inventory file only after you confirm the import. The cache itself remains read-only.

Close Firestorm before importing or refreshing if you want its latest saved changes. When the viewer is running, those changes may not have reached the cache file yet.

## Live viewer connection

Live viewer control is currently available on Windows only. From Home, choose **Launch Firestorm** and select the stock Firestorm executable. Close all Firestorm windows first: the LEAP option is read only at startup.

When the connection is ready, Shelfmark reads the active avatar UUID and can select the matching imported snapshot. LEAP does not identify the grid, so confirm Firestorm is on the Second Life main grid before using a live action.

## Move an item to Trash

To send an item to Trash:

1. Select the matching avatar snapshot in Shelfmark.
2. In Firestorm, keep the **All Items** inventory tab open and clear its inventory search field.
3. Select one item in Shelfmark and choose **Move to Trash**.
4. Shelfmark sends Firestorm its normal Delete action and checks the item’s parent folder before reporting success.
5. Close Firestorm and refresh the snapshot after the viewer has saved its cache.

Trash actions require a matching live Firestorm session on the Second Life main grid. Shelfmark does not edit the cache file. The Keep safe control is a local review reminder; it does not change inventory or prevent a live action.

Moves to ordinary folders remain paused. Stock LEAP does not reliably confirm which item is on the inventory clipboard after Cut, so Shelfmark cannot safely rule out pasting an older clipboard item. Make those moves in Firestorm.

The macOS and Linux builds can import and browse local snapshots, but do not currently support launching Firestorm through Shelfmark or using live Trash actions.

Firestorm and Second Life are named only to describe compatible software and files. Shelfmark is an independent project and does not include Firestorm code or assets.
