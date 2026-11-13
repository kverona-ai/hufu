# ADR-0013: Hufu updates itself with the system's download and archive tools, and checks the result itself

| | |
|---|---|
| **Status** | Accepted. Adds to [ADR-0009](0009-synchronous-core-and-own-protocol-clients.md), whose dependency list stays as it is |
| **Date** | 2026-11-13 |

## Context

Upgrading Hufu meant running the install script again and then `hufu doctor`. That is two commands
in two languages, it restarts nothing, and the script it runs is the one on the `main` branch, not
the one that belongs to a release. Most installations are done by an agent on the user's behalf
([ADR-0012](0012-connected-agents-may-configure-hufu.md)); an agent needs one command that asks no
question and whose result it can read.

Three things constrain how the program can update itself:

- [ADR-0009](0009-synchronous-core-and-own-protocol-clients.md) fixed a short dependency list.
  Every dependency has to build for six targets and into a static Linux binary, with the linker
  that ships with Rust. Hufu links no network library: it talks to agents over pipes.
- An update replaces the program that is running, possibly while jobs, a daemon and MCP servers
  that agents started are running from the same file. Windows does not allow a running program to
  be overwritten or deleted.
- An update that goes wrong must leave the installed program as it was.

## Decision

1. **`hufu update` is part of the binary.** `--check` only reports, `--version <v>` installs a
   named release (also an older one), `--from-source` builds with cargo, `--force` installs again.
2. **Downloads and archives are handled by the system's own tools.** A download is one run of
   `curl`, or of `wget` where there is no `curl`; an archive is unpacked by `tar`. Windows has
   shipped `curl.exe` and a `tar.exe` that reads zip archives since Windows 10 version 1803; the
   copies in `System32` are used, whatever else is on `PATH`. No HTTP, TLS or archive library is
   linked.
3. **What the safety of an update rests on is done by Hufu itself.** The SHA-256 sum of the
   archive is computed by code in `hufu-core` and compared with the release's manifest. The new
   program is then started once with `--version`: it must run on this machine and report the
   version the release names, before it replaces anything.
4. **A release is described by one file, `hufu-manifest.json`**, published next to the archives:
   the version, the tag, and the SHA-256 sum of every archive. The newest release is found by
   reading that file from `releases/latest/download/`. The archive is then fetched from the tag
   the manifest names, so a release published in between cannot mix two versions. A mirror or a
   local directory (`HUFU_DOWNLOAD_BASE`) serves the same files.
5. **Replacement is a rename.** The new program is written next to the old one and renamed over
   it. On Windows the old program is first renamed aside (`hufu.old-<id>.exe`), which is allowed
   while it runs, and removed once nothing runs from it.
6. **After the swap, everything is done by the new program**: the daemon is restarted if one was
   running, and `hufu doctor` is run. Running jobs finish with the program they started with.
7. **A process that still runs the old program starts its helpers from the new one.** After a
   replacement Linux names the old file `<path> (deleted)`; the lookup of the program to start
   runners with accounts for that. An MCP server that an agent started before the update keeps
   working.
8. **Without a release, the update builds from source** with `cargo install` into a scratch
   directory, and installs the result the same way. The same happens for a release that has no
   binary for the platform.

## Alternatives considered

| Option | Why not |
|---|---|
| An HTTP client crate with a TLS stack | The largest dependency Hufu would have, for one command. TLS libraries bring C or assembly code that needs a cross toolchain for the ARM64 and musl targets, which would end the "one runner, the linker that ships with Rust" release build |
| A self-update crate | Brings an HTTP client, an async runtime and archive libraries with it |
| Have `hufu update` download and run the install script | Two implementations in two shells of the part that matters most; the script would come from `main`, not from the release; no machine-readable result; PowerShell execution policy on Windows |
| Ask the GitHub API for the latest release | 60 requests per hour and address without a token; a mirror cannot serve it. The manifest is a plain file like the others |
| Overwrite the program in place | Not possible on Windows while it runs, and not atomic anywhere |

## Consequences

- `hufu update` needs `curl` or `wget`, and `tar`, on the machine. Every supported system has them
  by default except Windows before version 1803, where the install script remains the way to
  upgrade. The message says so.
- Every release carries `hufu-manifest.json`. The release workflow writes it and then checks, on
  all three systems, that `hufu update` can read the release before it is published. A mirror has
  to carry the manifest too.
- The checksum and the archive come from the same place, so this protects against damaged and
  mixed-up downloads, not against a release that was replaced by someone who controls the
  repository. That is the same trust as the install scripts have. Signing releases is open.
- There is one release channel. While no stable release exists a pre-release is what `latest`
  points to; afterwards pre-releases are installed by name.
- A process started before an update keeps running the old program until it ends. It starts new
  runners from the new one, so the job store format has to stay readable across the versions that
  can meet this way.
