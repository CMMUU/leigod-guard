param([string]$Target = 'x86_64-pc-windows-msvc')
$ErrorActionPreference = 'Stop'
# Only isolated off-screen test HWNDs receive synthetic messages. This never
# shuts down Windows, opens the user's app, or contacts a real Leigod account.
& cargo run --locked --offline --target $Target --example session_end_probe
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& cargo run --locked --offline --target $Target --example session_end_probe -- --manual
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& cargo run --locked --offline --target $Target --example session_end_probe -- --opened
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
