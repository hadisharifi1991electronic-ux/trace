---
name: hadi-working-style
description: How Hadi wants Claude to work on Trace: act without asking, install builds yourself, keep going
metadata:
  type: feedback
---

Hadi (2026-11-05): "don't ask about permission always click allow". They want Claude to act on its own: build, install on their PC, open the app, without asking first. Earlier: "do it your self".

**Why:** Hadi is often away and finds permission questions slow the work down.
**How to apply:** Don't ask before reversible steps (builds, installs into ...\Altium\Trace, sample files). Claude cannot approve the app's own permission prompts for Hadi; those are theirs. Still never modify or read Hadi's design files without them pointing to the file (see [[pcb-tool-project]]).
Install note (2026-11-12): device_commit_files now 404s on /mnt/project-files paths (needs a SendUserFile uuid, not available), so Hadi had to download the exe from the thread. Also it rejects files over 25 MB, so release builds are stripped (workspace profile.release strip = true); trace-app.exe is about 20 MB.
Release naming (Hadi 2026-11-05: "instead of label them as new put version"): each install goes in as trace-app-<version>.exe (workspace version in Cargo.toml, bump per release, CHANGELOG.md), never ".new"/".old". Desktop shortcut Trace.lnk is repointed to the newest exe (needs Remote Control). CLI stays trace.exe. 0.5.0 = constraint manager. Hadi (14:34): remove previous versions, so each install recycles the older trace-app-*.exe (via Remote Control, Recycle Bin). Roadmap agreed: impedance/stackup (0.7.0), then routing.
Language: Hadi sometimes writes in Persian (Farsi); answer in the language of their message.
Roadmap note (2026-11-06, told Hadi): to be "professional" Trace still needs fab outputs (Gerber/drill/ODB++/PnP/BOM), schematic editing + ECO, footprint library editor, fuller DRC (silk/mask/hole-hole/rooms), blind/buried vias. Recommended fab outputs after length matching.
Hadi (2026-11-12): "When this is done go for the next step": after installing a release, start the next roadmap item without waiting for "next".
