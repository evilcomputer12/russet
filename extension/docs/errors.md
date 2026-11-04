# Error reference

Every Russet error has a code made of a family and a name, such as `build.compileFailed`. Error
notifications show it in parentheses at the end, after the message and Russet's suggestion. The
**Open Docs** button on a notification opens the code's section of this page.

| Family | Area |
| --- | --- |
| `core` | general failures |
| `ipc` | the connection between the IDE and Russet's engine |
| `perm` | permissions and approvals |
| `config` | project and engine configuration files |
| `secrets` | the engine's secret store |
| `toolchain`, `sdk` | Swift and the iOS SDK |
| `device`, `app` | iPhones and the apps on them |
| `build` | building your app |
| `sign`, `account`, `provision` | signing, your developer account and provisioning |
| `gsa`, `anisette` | Apple ID sign-in |
| `debug`, `crash` | debugging and crash reports |
| `ship` | archives |
| `screen` | the iPhone's screen |

Many suggestions in Russet's messages mention `russet …` commands. Those belong to the engine's command
line. In the IDE, use the steps on this page instead.

## core.internal

Something failed that should never fail: a bug in Russet.

What to do: report it with the error message, the steps that led to it, and the versions from
**Russet: About Russet**. See "Report a bug" in [Troubleshooting](troubleshooting.md).

## core.cancelled

The operation was cancelled, for example with **Cancel** on its notification, or because the engine stopped
it.

What to do: nothing. Start it again when you want to.

## core.timeout

An operation took too long and was stopped.

What to do: try again. If it keeps happening, run **Russet: Run Doctor**, and look at the engine's log (see
[Troubleshooting](troubleshooting.md)).

## core.io

Russet could not read or write a file. The message names the file and the reason.

What to do: check that the file or folder exists, that your user can read and write it, and that its drive
has free space.

## core.diskFull

There is not enough free space for the operation. The message says on which drive, and how much is needed.

What to do: free space on that drive, then try again. **Russet: Clean Build Folder** removes the open
project's build files.

## core.notImplemented

The feature exists in Russet's interface but not yet in this release of its engine.

What to do: nothing for now; a later release adds it. See "Commands that are not available yet" in
[Troubleshooting](troubleshooting.md).

## core.invalidArgument

A value given to Russet's engine is not valid. The message says which value and why.

What to do: correct that value and try again. If the value came from the IDE itself rather than from you,
report it as a bug.

## core.notFound

Something Russet looked for does not exist, such as a file, a registered bundle identifier or a provisioning
plan. The message names it.

What to do: check the name in the message. For a provisioning plan, plan again: plans expire after
10 minutes.

## core.busy

Another job holds what this operation needs, for example the engine while it still has jobs running.

What to do: wait for the other job to finish, then try again.

## core.unsupportedPlatform

This operation is not supported on this platform yet.

What to do: nothing in this release. The message says what is missing.

## ipc.protocolMismatch

The IDE and the running engine speak different versions of their protocol, usually after an update.

What to do: run **Russet: Restart Russet Daemon**. If that does not help, close Russet and open it again:
Russet copies the matching engine into WSL when it starts.

## ipc.daemonUnavailable

The IDE could not reach Russet's engine, or could not start it.

What to do: follow "Russet's engine is offline" in [Troubleshooting](troubleshooting.md).

## ipc.daemonStarting

The engine is still starting.

What to do: wait a moment and try again.

## ipc.frameTooLarge

A single message to or from the engine was larger than 16 MiB, and the connection was closed. This usually
comes from an agent that sends a large amount of data at once.

What to do: pass large data through files instead.

## ipc.connectionClosed

The connection to the engine closed before the answer arrived, for example because the engine stopped.

What to do: try again. The IDE reconnects by itself. If it keeps happening, look at the engine's log (see
[Troubleshooting](troubleshooting.md)).

## ipc.buildMismatch

A different build of the engine is running than the one the IDE expects, for example after an update.

What to do: let running builds and runs finish, then run **Russet: Restart Russet Daemon**, or close Russet
and open it again.

## perm.denied

The caller is not allowed to do this. Agents see it most often: their permissions do not include the call,
for example uninstalling an app that does not belong to the project.

What to do: do it yourself in the IDE, or ask the agent to use another tool.

## perm.confirmationDeclined

Someone declined the confirmation Russet asked for.

What to do: nothing. Run it again if you meant to confirm.

## perm.confirmationUnavailable

The operation needs a confirmation, and nobody could be asked, for example because an agent started it
without the IDE.

What to do: run the operation from the IDE, which can ask you.

## perm.authFailed

The engine did not accept the connection, for example because its endpoint belongs to another Windows or
Linux user.

What to do: close Russet and open it again. If it continues, report it.

## perm.approvalRequired

The operation would change your Apple developer account or publish something, and needs a person's
approval first. Russet raises it when:

- an agent calls a tool such as `plan_apply`, `ship_upload` or `testflight_distribute`;
- Run needs a change in your account first, such as registering the iPhone or creating a certificate or
  provisioning profile ("provisioning needs … Apple-side change(s)").

What to do: this release of the IDE does not show approval requests yet, so it cannot grant one. When Russet
already has a matching profile for the app and the iPhone, Run does not need an approval. Report your case,
with the message, so it can be followed up.

## perm.approvalInvalid

The approval given with the call has expired, was already used, or was granted for different arguments.
Approvals expire 10 minutes after they are requested.

What to do: make the call again without the old approval, to request a new one.

## perm.rateLimited

A tool was called more often than its limit allows.

What to do: wait a little, then try again.

## perm.tierExceeded

The agent asked for a tool above the level its permissions allow.

What to do: if you agree that this agent may use such tools, raise its `tier-max` in the engine's
`mcp-policy.toml`, in `\\wsl.localhost\Russet-Dev\home\russet\.russet\ide\config`. Otherwise do the step
yourself.

## config.invalid

A configuration file is not valid. The message names the file, usually with the line and column. It can be
your project's `russet.toml` or `russet.local.toml`, an `.entitlements` file, or one of the engine's own
files.

What to do: fix the file at that position. An `.entitlements` file must be a property list whose top level is
a dictionary.

## config.projectNotFound

Russet found no project in the folder: no `russet.toml`, and no `Package.swift`.

What to do: open the project's folder, the one that holds `russet.toml` or `Package.swift`. To start a new
app, run **Russet: New Project…**.

## config.unknownKey

A configuration file has a key Russet does not know. The message names the file, the key and its position.

What to do: remove or rename the key. Keys are written in lower case with hyphens, such as `bundle-id`.

## config.readOnly

The setting cannot be changed, such as the file-format version `schema`, which Russet writes itself.

What to do: leave that line as it is.

## secrets.unavailable

The engine has no secret store to keep your sign-in and keys in.

What to do: close Russet and open it again; Russet starts its engine with its store. If it continues, report
it.

## secrets.locked

The engine's secret store does not open with the key it was given. This happens when
`%LOCALAPPDATA%\Russet\IDE\backend-secrets.dpapi` was deleted or replaced, or comes from another Windows
user.

What to do: do not delete that file. If it is gone, Russet cannot read what it stored before; report the
error, since the IDE has no command to reset the store yet.

## secrets.notFound

A secret Russet expected, such as an API key or an Apple ID session, is not in the store.

What to do: sign in again, or add the App Store Connect API key again. See [Signing](signing.md).

## secrets.passphraseRequired

The engine was started without the key to its secret store.

What to do: close Russet and open it again. Russet starts the engine with the key.

## toolchain.missing

A tool Russet needs, such as Swift, LLDB or git, is not installed in the backend.

What to do: open the setup page and make sure the **Swift toolchain** step is done. Then run
**Russet: Run Doctor**.

## toolchain.versionMismatch

The installed tool's version is not the one required, for example by a `[toolchain]` pin in your project's
`russet.toml`.

What to do: change or remove the pin in `russet.toml` so that it allows the installed version. The message
names the tool and both versions.

## toolchain.broken

A tool is installed but does not start or does not report its version.

What to do: run **Russet: Run Doctor** for details. If Swift is broken right after a Swift update, roll back
from the update's notification.

## toolchain.downloadFailed

A toolchain download failed.

What to do: check the connection and try again. An interrupted download resumes where it stopped.

## toolchain.verifyFailed

A download did not match its published checksum or signature, so Russet deleted it.

What to do: try again. If it happens again, report it.

## toolchain.pinsInvalid

Russet's list of supported tool versions is invalid, or does not cover the Swift or Xcode version in use.

What to do: run **Russet: Run Doctor**, and report the error with its message.

## toolchain.linkerUnsupported

The iOS SDK Russet built has no linker that can link for it.

What to do: report the error with its message and the Xcode version you used.

## sdk.missing

There is no usable iOS SDK, or none matches the version your project asks for.

What to do: open the setup page and set up the **iOS SDK from Xcode** step. If your project's `russet.toml`
names an Xcode SDK under `[toolchain]`, make it match the SDK you built.

## sdk.incompatible

The iOS SDK needs a different Swift version than the one installed. The message names both.

What to do: install the Swift version the SDK needs, or build the SDK from the Xcode that matches your
Swift. Russet knows the SDKs of Xcode 26 and 27.

## sdk.importFailed

Russet could not read what it needed from an Xcode archive or folder. The message says what and why.

What to do: check that the file is complete, for example by downloading it again, then try again.

## sdk.incomplete

The iOS SDK lacks a part your app needs, such as a library for an older iOS version.

What to do: build the SDK from a complete Xcode archive, or raise your app's minimum iOS version.

## device.notFound

No iPhone matches: none is connected, or the selected one is no longer attached.

What to do: connect and unlock the iPhone, then select it with **Russet: Select Run Destination**. When
several are connected, choose one there.

## device.notPaired

The iPhone is not paired with this PC.

What to do: run **Russet: Pair…**, unlock the iPhone and tap **Trust**.

## device.locked

The iPhone is locked.

What to do: unlock it, and keep it unlocked while Russet installs, runs or debugs.

## device.untrusted

The iPhone refused to trust this PC, usually because **Don't Trust** was tapped.

What to do: on the iPhone, reset Location & Privacy, then run **Russet: Pair…** again and tap **Trust**.

## device.developerModeDisabled

Developer Mode is off on the iPhone.

What to do: on the iPhone, open Settings › Privacy & Security › Developer Mode, turn it on and restart the
iPhone.

## device.ddiNotMounted

The iPhone has no Developer Disk Image mounted. Debugging and some other features need it.

What to do: run **Russet: Mount Developer Disk Image**.

## device.tunnelFailed

Russet could not open its connection tunnel to the iPhone.

What to do: try again. If it keeps failing, unplug and reconnect the cable, then run **Russet: Run Doctor**.

## device.disconnected

The iPhone went away during the operation.

What to do: reconnect it and try again.

## device.usbmuxUnavailable

Russet cannot reach iPhones. On Windows this usually means Apple Devices was not running when Russet
started, so Russet's engine started without iPhone support.

What to do:

1. Open Apple Devices from the Start menu (install it from the Microsoft Store if needed).
2. Close Russet and open it again.

## device.trustPending

The iPhone shows the Trust dialog and has not answered it.

What to do: unlock the iPhone and tap **Trust**.

## device.ddiPersonalizationFailed

Apple's signing server did not personalize the Developer Disk Image for this iPhone.

What to do: check that this PC can reach Apple (gs.apple.com), and that the disk image matches the iPhone's
iOS version. Try again.

## device.tunnelStalled

The connection to the iPhone stopped moving.

What to do: try again. If it happens again, reconnect the cable, and report it.

## device.serviceUnavailable

A service Russet needs on the iPhone did not answer.

What to do: run **Russet: Mount Developer Disk Image**. If the disk image is mounted, this iOS version does not
offer the service.

## device.serviceNotImplemented

This iOS version, or this Russet release, does not implement the feature.

What to do: nothing for now.

## device.timeout

The iPhone did not answer in time.

What to do: keep it unlocked and connected, and try again.

## device.unsupported

The operation is not supported for this iPhone, its iOS version, or the way it is connected.

What to do: nothing for now. The message says what is not supported.

## device.io

Reading from or writing to the iPhone failed.

What to do: try again. Reconnect the iPhone if it happens again.

## device.protocol

The iPhone answered something Russet did not expect.

What to do: try again. If it continues, report it with the message.

## device.reserved

Another client, such as an agent, has reserved this iPhone.

What to do: wait until it is done with the iPhone, then try again.

## device.liveBackendRefused

Another program already owns the connection to your iPhones. Only one Russet engine at a time may use them,
so this happens, for example, when a Russet release and a Russet development installation run at the same
time.

What to do: close the other Russet, then close this one and open it again.

## device.ddiSourceMissing

Russet has no Developer Disk Image it can mount on this iPhone.

What to do: this release cannot import one from Xcode yet. You can point the engine at a copy of a Mac's
`/Library/Developer/DeveloperDiskImages/iOS_DDI` folder: set `dir` under `[device.ddi]` in the engine's
`config.toml`, in `\\wsl.localhost\Russet-Dev\home\russet\.russet\ide\config` (create the file if it does
not exist), to that folder's path as WSL sees it, such as `/mnt/c/...`.

## device.ddiIncompatible

The Developer Disk Image does not fit this iPhone's iOS version.

What to do: use the disk image of an Xcode that supports this iOS version.

## device.ddiMirrorConsentRequired

The engine is configured to take Developer Disk Images from a third-party mirror, which needs your consent.

What to do: remove that source from `[device.ddi]` in the engine's `config.toml`, or set `allow-mirror = true`
only if you accept Apple files from an unlicensed third-party mirror.

## device.fallbackUnavailable

The fallback source of Developer Disk Images is not available in this release.

What to do: nothing for now.

## app.notInstalled

The app is not installed on the iPhone.

What to do: run it from Russet, which installs it first.

## app.installFailed

The iPhone refused to install the app. The message gives the iPhone's reason. Common reasons are a signature
problem, a provisioning profile that does not list this iPhone, a minimum iOS version above the iPhone's, or
an App ID that does not match.

What to do: check that the iPhone is the selected run destination, that the app's minimum iOS version is
not above the iPhone's, and that the bundle identifier in `russet.toml` is yours. Then run again.

## app.launchFailed

The app was installed but did not start.

What to do: unlock the iPhone, and run again. If it keeps failing, check the Device Console for the app's
output.

## build.projectAmbiguous

The project folder matches more than one kind of project, so Russet does not know how to build it.

What to do: set `stack` under `[project]` in `russet.toml`.

## build.inProgress

A build of this project is already running.

What to do: wait for it to finish, or cancel it with **Cancel** on its notification.

## build.compileFailed

Compilation failed.

What to do: fix the errors listed in the Problems view (Ctrl+Shift+M); the Russet Build output has the
compiler's full output. If the message says the compiler "was stopped before it finished (WSL ran out of
memory)", stop the virtual iPhone or other memory-hungry apps, or raise WSL's memory limit with
**Russet: Change WSL Limits (Memory and Disk)…**, then build again.

If it fails before compiling with a message about `Package.swift`, check that `Package.swift` and the
package's dependencies are valid.

## build.linkFailed

Linking failed, for example because of an undefined symbol.

What to do: fix the errors in the Problems view and the Russet Build output. If the message says WSL ran out
of memory, follow the steps under `build.compileFailed`.

## build.bundleIdInvalid

The bundle identifier is not valid. Bundle identifiers use letters, digits, `.` and `-`, with no empty part,
such as `com.yourname.myapp`. An extension's identifier must start with its app's identifier and a dot.

What to do: fix `bundle-id` in `russet.toml`. See [Signing](signing.md).

## build.lintFailed

Russet's checks of the built app found problems that block the build.

What to do: fix the findings. They are listed in `build-report.json`, in your project's
`.russet\build\<configuration>-iphoneos` folder.

## build.packageFailed

Russet could not package the app into an `.ipa` file.

What to do: check the free space on the project's drive, then build again.

## build.scaffoldTargetNotEmpty

**Russet: New Project…** found files in the folder it would create the app in.

What to do: choose another location or name, so the new app gets an empty folder.

## build.scaffoldStackUnsupported

There is no project template for this kind of project yet. Russet creates Swift package apps today.

What to do: nothing for now.

## build.scaffoldNameInvalid

The app name has no letters or digits to make a name from.

What to do: choose a name with at least one letter or digit, such as "My App".

## sign.noIdentity

Russet has no usable signing identity (certificate and private key) for the account, or the identity's key is
missing from the secret store.

What to do: check that you are signed in, in the **Signing** view. Sign in again or add your API key again.
See [Signing](signing.md).

## sign.profileMismatch

No provisioning profile matches the app's bundle identifier, the team, the certificate and the iPhone.

What to do: check that the bundle identifier in `russet.toml` is yours, and that the right iPhone is the run
destination, then run again. See "When signing needs a change in your Apple account" in
[Signing](signing.md).

## sign.entitlementNotInProfile

The app asks for an entitlement that its provisioning profile does not grant.

What to do: remove the entitlement from the `.entitlements` file. To keep it, the App ID needs the matching
capability in Apple's developer portal, and the app needs a profile made after that.

## sign.entitlementValueNotAllowed

An entitlement has a value its provisioning profile does not allow.

What to do: change the value in the `.entitlements` file to one the profile grants. To keep the value, the
capability must allow it in Apple's developer portal, and the app needs a profile made after that.

## sign.entitlementUnresolvedVariable

An entitlement contains a `$(VARIABLE)` Russet cannot fill in.

What to do: use `$(AppIdentifierPrefix)`, `$(TeamIdentifierPrefix)`, `$(PRODUCT_BUNDLE_IDENTIFIER)` or a
literal value in the `.entitlements` file.

## sign.lintFailed

Russet's checks before signing found problems. Warnings do not block; errors do.

What to do: fix the problems the message lists, then run again.

## sign.codesignFailed

Signing the app failed.

What to do: run again, and run **Russet: Run Doctor**. If it continues, report it with the message.

## sign.profileInvalid

A provisioning profile cannot be read.

What to do: run again. If it continues, report it with the message: the IDE has no command to replace a
profile file yet.

## sign.bundleInvalid

What Russet was asked to sign is not a signable app: it lacks an `Info.plist`, a bundle identifier or an
executable.

What to do: build the app again, and sign the `.app` the build produced.

## sign.verifyFailed

The signed app does not pass Russet's signature check.

What to do: build and run again. If it continues, report it with the message, which lists each broken file.

## account.ascUnauthorized

App Store Connect rejected your API key.

What to do: check the key ID (in the file name `AuthKey_<KEYID>.p8`) and the issuer ID, and that the key was
not revoked. Check that Windows' clock is right. Then add the key again with
**Russet: Add App Store Connect API Key…**.

## account.ascForbidden

App Store Connect refused the request. Two causes are common:

- Apple requires an updated developer agreement. The account holder must sign in at
  https://developer.apple.com/account and accept the latest Apple Developer Program License Agreement.
- The key's role lacks the permission. Use a key with the Admin or App Manager role and access to
  Certificates, Identifiers & Profiles.

What to do: fix the cause the message names, then try again.

## account.ascScope

The API token Russet made does not cover the request. This is a bug in Russet.

What to do: report it with the message.

## account.ascRateLimited

App Store Connect limited your key's requests. It allows 3,600 requests per hour per key.

What to do: wait, then try again.

## account.individualKeyNoProvisioning

An individual API key cannot create certificates or provisioning profiles.

What to do: add a team key (with an issuer ID, and the App Manager or Admin role), or sign in with your
Apple ID. See [Signing](signing.md).

## account.ascAmbiguous

Several records in your developer account match, such as two App IDs with the same bundle identifier.

What to do: remove the duplicates in Apple's developer portal (Identifiers), then try again.

## account.appleServiceUnavailable

An Apple service did not answer, or answered something unexpected.

What to do: check the network and try again later. https://developer.apple.com/system-status/ shows Apple's
outages.

## account.keyInvalid

The file is not a readable App Store Connect key.

What to do: choose the `AuthKey_<KEYID>.p8` file App Store Connect gave you, unchanged.

## account.noAccount

No developer account is set up, or the one requested does not exist.

What to do: sign in with your Apple ID or add an API key. See [Signing](signing.md).

## provision.planStale

A provisioning plan expired or no longer matches your account. Plans can be applied for 10 minutes.

What to do: plan again, then apply the new plan.

## gsa.badCredentials

Apple did not accept the Apple ID or password.

What to do: check both and try again. An app-specific password does not work here.

## gsa.badCode

Apple did not accept the verification code.

What to do: enter the 6-digit code shown on your trusted device or sent by text message. If it expired,
request a new one.

## gsa.locked

Apple reports that this Apple ID is locked.

What to do: unlock it at https://iforgot.apple.com, then sign in again.

## gsa.identityRejected

Apple did not accept the sign-in identity of this PC (anisette data).

What to do: sign in through an identity server: set Russet › Account: Anisette Server
(`russet.account.anisetteServer`) to one you trust. With an identity server already set, try again later or
choose another. See [Signing](signing.md).

## gsa.codeRateLimited

Apple is not sending more verification codes for now: there were too many requests or attempts.

What to do: wait a while, then start the sign-in again.

## gsa.protocol

Apple's sign-in service answered something Russet cannot read.

What to do: try again. If it continues, update Russet; Apple may have changed something Russet relies on.

## gsa.rootPinMismatch

The Apple certificate Russet received does not match the one built into Russet. Something between this PC
and Apple changed it.

What to do: Russet will not sign in over this connection. Use another network.

## gsa.sessionExpired

Your stored Apple ID session has expired.

What to do: sign in again with **Russet: Sign in with Apple ID…**.

## gsa.loginExpired

The sign-in took longer than 10 minutes, or too many wrong codes ended it.

What to do: start the sign-in again.

## gsa.ds2Failed

Apple's developer services refused a request.

What to do: check your team's status at https://developer.apple.com/account: an agreement to accept, or a
membership that expired. Then try again.

## anisette.libsMissing

Apple's sign-in support libraries, which identify this PC, are not downloaded.

What to do: start the sign-in again and accept the download on the first page (about 2 MB).

## anisette.pinMismatch

The downloaded sign-in support libraries do not match the versions Russet expects, so Russet refuses to run
them.

What to do: try again on another network. If it continues, report it. An identity server avoids the
libraries; see [Signing](signing.md).

## anisette.adiFailed

The helper that runs Apple's sign-in support libraries failed, or its stored identity cannot be read.

What to do: try again. If it continues, reinstall Russet, or sign in through an identity server.

## anisette.consentRequired

Downloading Apple's sign-in support libraries needs your consent.

What to do: start the sign-in again and accept the download.

## anisette.fetchFailed

Downloading Apple's sign-in support libraries failed.

What to do: check the network and try again. Russet only reads about 2 MB.

## anisette.remoteFailed

The identity server in `russet.account.anisetteServer` failed or answered something unreadable.

What to do: check that the server runs and is reachable, choose another one, or clear the setting to use this
PC's own identity.

## anisette.unsupported

This PC cannot create its own sign-in identity.

What to do: sign in through an identity server, or add an App Store Connect API key. See
[Signing](signing.md).

## anisette.provisioningFailed

Apple refused to set this PC up for signing in.

What to do: try again later, or sign in through an identity server.

## debug.attachDenied

The iPhone refused to let the debugger attach.

What to do: debug a **Debug** build signed with a development profile. Release and distribution builds
cannot be debugged.

## debug.alreadyDebugged

Another debugger is already attached to the app.

What to do: stop the other debug session, then try again.

## debug.debugserverUnavailable

The iPhone's debug server could not be reached.

What to do: run **Russet: Mount Developer Disk Image**, then **Russet: Run Doctor**.

## debug.lldbUnsupported

The debugger Russet found cannot debug iPhone apps, or `lldb-dap` is missing.

What to do: make sure the setup page's **Swift toolchain** step is done; Russet uses Swift's LLDB. Then run
**Russet: Run Doctor**.

## debug.handoffTimeout

The app was launched for debugging, but the debugger did not connect in time, so Russet stopped the app.

What to do: start the debug session again.

## debug.processExited

The app exited before the debugger attached, or while it attached.

What to do: check the Device Console for the app's output, then try again.

## debug.symbolsNoSpace

There is not enough free space for the iPhone's symbol files.

What to do: free space, then try again.

## debug.symbolsService

Russet cannot fetch the iPhone's symbol files in this release.

What to do: nothing for now.

## debug.brokerLost

The debug session lost its connection to the iPhone.

What to do: reconnect the iPhone and start the debug session again.

## debug.sessionNotFound

The debug session no longer exists.

What to do: start a new one.

## crash.notFound

No crash report has that ID.

What to do: list the crash reports again and use an ID from the list.

## crash.symbolicationFailed

A crash report could not be read: it is not text, or not the expected format.

What to do: fetch the report again; it may be cut short.

## ship.archiveFailed

Russet could not create the archive or its debug symbols.

What to do: check that the build product exists and is readable, and that the drive has free space.

## ship.ipaInvalid

What Russet was asked to package is not a usable `.app` or archive, or the `.ipa` could not be written.

What to do: build the app again, and check that the output folder is writable.

## screen.streamFailed

The iPhone did not start streaming its screen.

What to do: unlock the iPhone and try again. The iPhone view shows screenshots meanwhile.

## screen.codecUnsupported

The screen stream needs HEVC video, which is not available here.

What to do: nothing. The iPhone view shows screenshots instead.
