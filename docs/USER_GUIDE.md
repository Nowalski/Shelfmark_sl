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

Live Firestorm control is Windows-only. The packaged app can connect to stock Firestorm and read the active avatar. To move items from a packaged release, install the optional [Firestorm Mini viewer](https://github.com/Nowalski/Shelfmark_sl/releases/tag/firestorm-mini-7.2.5.81788). It uses a separate viewer channel so it can be installed alongside stock Firestorm. The open build has no FMOD sound or voice; stock LEAP move actions remain enabled only in development builds.

1. Install Firestorm Mini from the pre-release above, then close every Firestorm window.
2. In Shelfmark Home, choose **Firestorm Mini** and select the installed `Firestorm.exe`.
3. Sign in to the Second Life main grid, then select the matching avatar snapshot in Shelfmark.
4. In Firestorm, open **All Items** and clear its inventory search field.
5. Select one item in Shelfmark and choose **Move to Trash**. Firestorm displays a confirmation for that item; Shelfmark waits for the live folder to change before updating its catalogue.
6. When finished, close Firestorm and choose **Refresh snapshot** so the catalogue reflects the cache Firestorm saved. If Mini uses a separate cache folder, choose **Browse** and import its saved snapshot.

The bridge checks the live avatar and the item’s current source folder against your snapshot. It moves one item at a time and uses Firestorm’s normal removable-item checks. It only targets Trash on the Second Life main grid. Moves to ordinary folders remain paused; do those in Firestorm. The viewer is experimental and does not include sound or voice.

## Browser preview

The browser preview runs with `npm run dev` and contains fictional sample data. It cannot read files from your computer or connect to Firestorm. Use the installed desktop app to import a real snapshot.

## Updates and local data

The desktop app checks this repository’s GitHub Releases for signed updates. You can also check manually under **Settings → Desktop app updates**. Shelfmark does not send inventory, notes, tags, or photos to a Shelfmark service.

The local SQLite index and WebView profile are stored in your operating system’s app data folders. Export your personal profile before moving devices or reinstalling if you want to keep your notes, tags, collections, and photos.
