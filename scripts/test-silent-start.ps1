param([string]$Target = 'x86_64-pc-windows-msvc')
$ErrorActionPreference = 'Stop'
# The probe uses a tiny off-screen eframe window with no account/configuration,
# tray, monitor or production instance mutex. Never start the user's application.
& cargo run --locked --offline --target $Target --example silent_start_probe
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& cargo run --locked --offline --target $Target --example silent_start_probe -- --manual
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
