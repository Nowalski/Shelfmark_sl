# Frequently asked questions

## Does Shelfmark change my inventory?

Importing and browsing are read-only. On Windows, you can explicitly send one selected item to Firestorm’s Trash through the matching live Firestorm session on the Second Life main grid. Shelfmark checks that the item moved before updating the local catalogue. It does not delete items permanently or make background changes.

Moves to ordinary folders are paused because Firestorm’s LEAP interface cannot reliably confirm the clipboard item. Make those moves in Firestorm.

## Is Trash available on macOS or Linux?

Not at this time. The macOS and Linux desktop builds can import and browse local snapshots. Launching Firestorm through Shelfmark and live Trash actions are currently Windows-only.

## Does “Keep safe” protect an item from being moved?

No. Keep safe is a local review reminder. It does not change Firestorm inventory and does not block a Trash action. Review the item in Firestorm before changing it.

## Are candidate groups accurate?

They are clues based on names, folder paths, item types, or repeated names in the snapshot. A possible duplicate may have different contents, a package may still be useful, and Shelfmark does not inspect notecard contents. Treat every group as a place to review, not a recommendation to delete.

## What does Recently added mean?

It uses timestamps found in the cache. It does not represent purchase history, and an old or incomplete cache may not include recent changes.

## Does Shelfmark upload my inventory or photos?

Shelfmark has no service that receives your inventory, notes, tags, labels, or photos. Those stay in the local app data and profile. The desktop app contacts GitHub Releases when checking for signed updates.

## Can I move my setup to another computer?

Use **Settings → Personal data file → Export profile** on the old device, then restore the file on the new one and import the matching Firestorm cache. The profile contains your labels, notes, tags, saved searches, collections, keep-safe marks, and photos. It does not contain an inventory cache or catalogue index.

## Why do results not match Firestorm right now?

Shelfmark searches the imported snapshot, not Firestorm live. Close the viewer, refresh the snapshot, and check that you imported the right avatar. The original cache is never modified.

## Does the browser preview use my real inventory?

No. The preview uses fictional sample data. Only the installed desktop app can read a local Firestorm cache.

## Is Shelfmark open source?

The source is public, but Shelfmark uses a personal-use license. It is not an OSI-approved open-source project. The license allows personal, non-commercial use and private modifications; it does not grant permission to sell or redistribute copies outside GitHub. Public repository viewing and forking are also subject to GitHub’s terms. See [LICENSE](../LICENSE) for the terms.

The earlier 0.1.0 release was published with MIT terms. Removing that release and tag from the official repository cannot revoke rights for copies already obtained; those copies remain governed by the license they included. Shelfmark 0.1.1 and later use the personal-use terms.

## Can I contribute a code change?

Bug reports and suggestions are welcome. Code pull requests are not currently accepted while the project uses personal-use terms. See [Contributing](../CONTRIBUTING.md).
