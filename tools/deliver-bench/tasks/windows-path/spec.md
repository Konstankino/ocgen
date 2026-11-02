# What the user knows

- The paths come from Windows users' reports, and the tool reading them runs on macOS and Linux as well as Windows. The answer has to be right on all of them.
- Windows ignores case in the whole path, not only in the drive letter. The part of the path that remains keeps the case it came with.
- Paths may mix `\` and `/`, may hold spaces and `..` segments, and a root may end in a separator.
- A path outside the root comes back unchanged. That includes a folder that only shares a prefix with the root (`C:\proj2` under `C:\proj`) and a path on another drive.
- macOS and Linux paths have to keep working.
