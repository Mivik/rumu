---
description: Why rm -rf rooted at / is blocked unconditionally, and what to do instead.
---
Commands like `rm -rf /` (or any rm -rf rooted at `/`) can brick the machine —
they wipe the system, mounted media, and backups alike. They are blocked
unconditionally: destructive deletes must target a concrete directory inside a
workspace, and list what will be deleted first. If a wide cleanup is
really intended, list what will be deleted first (`find <dir> -maxdepth 2`)
and get the user's confirmation.
