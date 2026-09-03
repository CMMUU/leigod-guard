# Windows installer

Build through `scripts/installer.ps1`. The installer requires Inno Setup 6.4 or
newer and these compile-time defines:

| Define | Input |
| --- | --- |
| `AppVersion` | The Cargo package version, for example `0.5.1` |
| `SourceDir` | An isolated directory containing the audited portable payload |
| `OutputDir` | Destination directory for the generated setup executable |
| `BootstrapperFile` | Microsoft's unmodified, signature-verified WebView2 bootstrapper |

The output is `leigod-guard-v<VERSION>-windows-x64-setup.exe`. Do not build from a
directory containing personal configuration or use a different source of the
Microsoft bootstrapper.

The stable application identifier is
`{6ABF5F53-AFD4-4D24-A930-174CF6B21B7A}`. Setup installs for the current user,
defaults to `%LOCALAPPDATA%\Programs\LeigodGuard`, and preserves the previous
installation directory during upgrades. It never enables startup for a new
user. An existing `LeigodGuard` startup entry whose executable is
`leigod-guard.exe` is migrated to the installed program. Uninstall removes that
entry only if it still points to this installation, and does not remove
`%APPDATA%\leigod-guard` or shared WebView2 components.

`AppMutex=Local\LeigodGuard` prevents installing over or uninstalling an active
version that supports the mutex. `CloseApplications=no` disables automatic
process closing in both interactive and silent installations. Version 0.5.0
portable copies predate this mutex and must be exited before installation.

The prerequisite code checks the documented WebView2 `pv` version in the user
and machine registry views. If missing, it runs the included bootstrapper with
`/silent /install`, waits, and checks both the process result and the installed
version. Failure prevents the app from being installed. Interactive users can
retry or cancel; silent installations return a failure exit code. A prerequisite
that requires restart does not launch the app or report installation success.
The final launch checkbox is skipped for silent installation.

API references checked during implementation:

- [Microsoft WebView2 distribution](https://learn.microsoft.com/microsoft-edge/webview2/concepts/distribution)
- [Inno Setup prerequisite event](https://jrsoftware.org/ishelp/topic_scriptevents.htm)
- [Inno Setup application mutex](https://jrsoftware.org/ishelp/topic_setup_appmutex.htm)
- [Inno Setup process waiting](https://jrsoftware.org/ishelp/topic_isxfunc_exec.htm)
- [Inno Setup runtime extraction](https://jrsoftware.org/ishelp/topic_isxfunc_extracttemporaryfile.htm)
- [Inno Setup translated messages](https://jrsoftware.org/ishelp/topic_languagessection.htm)

`languages/ChineseSimplified.isl` is a project-maintained translation overlay.
It is loaded after the compiler's English message set to remain compatible
with newer compiler messages without fetching a translation during builds.
