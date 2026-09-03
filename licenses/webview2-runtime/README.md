# Microsoft Edge WebView2 Runtime

The Windows installer includes the unmodified Microsoft-signed Evergreen WebView2 Runtime Bootstrapper. When the Runtime is absent, this bootstrapper downloads and installs it from Microsoft. The bootstrapper and Runtime are Microsoft components; they are not relicensed under Leigod Guard's MIT license or the licenses of its Rust WebView2 bindings.

## License source

- [Microsoft WebView2 download page](https://developer.microsoft.com/en-us/microsoft-edge/webview2/): the Evergreen Bootstrapper download button displays the Microsoft Software License Terms for Microsoft Edge WebView2 Runtime.
- [Official license endpoint used by that download page](https://developer.microsoft.com/microsoft-edge/api/eula/webview2): the `evergreenHtml` field supplies those terms.
- Retrieved on **2026-09-03**, locale **en-us**. `LICENSE.html` preserves the returned HTML license text. `LICENSE.txt` contains the same text with HTML markup removed and whitespace normalized. `INSTALLER-LICENSE.txt` combines the application's MIT license, this separate Microsoft license, and the runtime privacy notice for the installation wizard.

Microsoft's terms govern these components. This directory records the terms supplied with this release; it does not grant additional rights over Microsoft software or its third-party components.

## Distribution and privacy references

- [Distribute your app and the WebView2 Runtime](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution): Microsoft's guidance describes packaging the Evergreen Bootstrapper with an application and running it when the Runtime is missing.
- [Data and privacy in WebView2](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/data-privacy).
- [Microsoft Privacy Statement](https://privacy.microsoft.com/en-us/privacystatement).
- [Microsoft Defender SmartScreen privacy details](https://learn.microsoft.com/en-us/microsoft-edge/privacy-whitepaper#smartscreen).

The embedded WebView2 component includes Microsoft Defender SmartScreen, which is enabled by default and collects and sends user information to Microsoft as described in the linked Microsoft Privacy Statement and SmartScreen privacy documentation. WebView2 also collects required diagnostic data; optional diagnostic data follows the applicable Windows diagnostic data setting. See the project's [privacy explanation](../../docs/PRIVACY.md).
