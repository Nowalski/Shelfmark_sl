# Using Shelfmark

Shelfmark turns a Firestorm inventory cache into a searchable catalogue stored on your device. It never asks for your Second Life password.

## Import a snapshot

1. Open the desktop app and choose **Import Firestorm cache** in the sidebar or Settings.
2. Shelfmark checks the usual cache folders and lists matching files. During discovery it reads file names, dates, and optional cached avatar names; it does not parse an inventory snapshot yet.
3. Select the avatar snapshot you want. Add an optional local label if the avatar name is unclear.
4. Choose **Import selected avatar** and confirm. Shelfmark then reads that cache and builds its local index.

Use **Browse** for a custom cache folder. Firestorm cache files usually end in `.inv.llsd.gz` or `.inv`. Close Firestorm before importing or refreshing when you need the latest saved snapshot. The cache is read-only; Shelfmark does not rewrite or delete it. You can import more than one avatar and switch snapshots from the avatar menu.

## Find and review items

Use **Find** to search item names, descriptions, folder paths, and your tags. Search does not include your private notes. Narrow results by item type, sort by name or cache timestamp, or open a saved search.

Use **Folders** to follow the original inventory tree. **Recently added** uses timestamps recorded in the cache, not purchase history. An older or partial snapshot can make recent items or other results incomplete.

**Candidate groups** are review clues, such as demo-like names, possible packages, body-fit or size terms, repeated names, notecards, and landmarks. The app does not inspect the contents of notecards or determine whether an item is still useful. Check an item in Firestorm before acting on a clue.

**Empty folders** shows folders that had no indexed contents in the selected snapshot. It is a review list; Shelfmark does not delete folders.

The **Keep safe** control and automatic safety clues are local reminders for review. They do not change inventory and do not block a Trash action.

## Keep personal notes and collections

Select an item to add your own tags and notes. Create collections to keep related items together, and add a cover or reference photo. These details belong to the local Shelfmark profile. When a Firestorm thumbnail is available, you can preview it locally and choose **Keep as item photo**.

To move your personal setup to another device, use **Settings → Personal data file → Export profile**. The profile file includes your local labels, tags, notes, saved searches, collections, keep-safe marks, and photos. It does not include the inventory cache or the catalogue index. On the new device, restore the profile and import the matching Firestorm cache.

## Move an item to Trash

Live Firestorm controls are currently available on Windows only. They require stock Firestorm on the Second Life main grid.

1. Close every Firestorm window before launching it from Shelfmark. Stock Firestorm reads the LEAP startup option only when it starts.
2. In Shelfmark, choose **Launch Firestorm** and select the matching avatar snapshot.
3. In Firestorm, open the **All Items** inventory tab and clear its inventory search field.
4. Select one item in Shelfmark and choose **Move to Trash**. Shelfmark asks Firestorm to use its normal Delete action for that item, then checks the item’s new location before updating the catalogue.
5. When finished, close Firestorm and choose **Refresh snapshot** so the catalogue reflects the cache Firestorm saved.

Use the Second Life main grid and make sure the connected Firestorm avatar matches the selected snapshot. Moves to ordinary folders are paused because stock LEAP cannot reliably confirm which item is on the inventory clipboard. Make those moves in Firestorm.

## Browser preview

The browser preview runs with `npm run dev` and contains fictional sample data. It cannot read files from your computer or connect to Firestorm. Use the installed desktop app to import a real snapshot.

## Updates and local data

The desktop app checks this repository’s GitHub Releases for signed updates. You can also check manually under **Settings → Desktop app updates**. Shelfmark does not send inventory, notes, tags, or photos to a Shelfmark service.

The local SQLite index and WebView profile are stored in your operating system’s app data folders. Export your personal profile before moving devices or reinstalling if you want to keep your notes, tags, collections, and photos.
