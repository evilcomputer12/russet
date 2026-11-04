# Signing

An iPhone only runs apps signed for it. Russet signs your apps with your Apple developer account. You can use
either kind of account, or both:

- **An Apple ID**, free or paid. A free Apple ID runs apps on your own devices for 7 days at a time. A paid
  Apple Developer Program membership signs for a year and can publish.
- **An App Store Connect API key**, which works with paid developer accounts.

## Sign in with your Apple ID

Russet signs in to Apple directly from this PC, as Xcode does on a Mac. Your password goes once to Russet's
engine on this PC. The IDE never stores it, and neither do your settings.

1. Run **Russet: Sign in with Apple ID…**, or select **Sign in with Apple ID…** on the setup page. The
   **Sign in with Apple ID** tab opens.
2. The first time, read the notes on the page. Russet downloads Apple's sign-in support libraries (about
   2 MB) to identify this PC to Apple. Tick **I understand and want to sign in with my Apple ID** and
   continue.
3. Enter your Apple ID (email or phone number) and your password. An app-specific password does not work
   here.
4. Apple shows a 6-digit code on your iPhone, iPad or Mac. Enter it.
   - No code? Select **Didn't get a code? Text me instead**, choose a phone number and select
     **Text Me the Code**.
   - To go back to a code on your devices, select **Use my trusted device instead**.
5. The page shows **You're signed in**. Russet can now sign and install your apps.

A sign-in that is not finished within 10 minutes expires. Enter your Apple ID and password again.

If you forgot your password, the page has a **Forgot Apple ID or password?** link to Apple.

### What Russet keeps

Russet keeps your Apple ID session, never your password, in its engine's encrypted secret store. The key
to that store is protected by Windows for your Windows user, in
`%LOCALAPPDATA%\Russet\IDE\backend-secrets.dpapi`. Do not delete that file: without it, Russet cannot open
what it stored, and you have to sign in again.

### Use an identity server

Apple asks every sign-in for an identity of the computer, called anisette data. By default this PC creates
its own identity locally.

If Apple does not accept this PC's own identity, the page says so and offers **Use an identity server…**.
It opens the setting `russet.account.anisetteServer` (Russet › Account: Anisette Server):

1. Set it to the `https://` address of an anisette server you trust.
2. Start the sign-in again.

The server's operator sees this computer's Apple device-identity requests. They do not see your Apple ID
password or your two-factor codes. The setting is read from your user settings only, so a project's
workspace settings cannot change it. The sign-in page names the server while one is in use.

### Turn Apple ID sign-in off

Set `russet.account.appleIdSignIn` to `off`. **Russet: Sign in with Apple ID…** then points you to the
setting. The default is `auto`. See [Settings](settings.md).

## Add an App Store Connect API key

1. In App Store Connect, open Users and Access › Integrations and create a key. To let Russet create
   certificates and profiles, give it the App Manager or Admin role.
2. Download the key. Apple names the file `AuthKey_<KEYID>.p8`. Keep that name: Russet reads the key ID from
   it.
3. Run **Russet: Add App Store Connect API Key…** and select the `.p8` file.
4. Enter the **Issuer ID**: the UUID shown above the keys on the same App Store Connect page.
   Leave it empty for an individual key. Apple does not let individual keys manage certificates or profiles.
5. Russet shows "Added API key" with the key's ID.

Press Escape at the Issuer ID to cancel without adding the key.

## See your accounts

- The **Signing** view in Russet's side bar lists your accounts, each with its kind, team and whether it is
  the default. **Russet: Refresh Signing** reads them again.
- The status bar shows the default account's team, or its name when Russet does not know the team yet. Click
  it, or run **Russet: Signing & Capabilities…**, to open the Signing view.
- On the setup page, the sign-in step shows the default account once you are signed in.

**Russet: Sign Out** and **Russet: Fix Signing** are not available yet: they say they are coming in a later
update.

## Choose your bundle identifiers

Apple registers a bundle identifier for one developer only, and never registers `com.example`.

### Your organization identifier

The first time Russet needs it, it asks for your organization identifier: a reverse-DNS prefix such as
`com.yourname`. It uses letters, digits and hyphens, with at least one dot. Russet saves it in the setting
`russet.project.organizationIdentifier` and suggests bundle identifiers under it, such as
`com.yourname.myapp` for an app named "My App".

### Replace a placeholder identifier

Before Run or Debug, Russet checks the app's bundle identifier. If it is a placeholder such as
`com.example.myapp`, Russet warns that Apple never registers it.

1. Select **Change Identifier**.
2. Enter your organization identifier if Russet asks for it.
3. Confirm or edit the bundle identifier.

Russet writes the new identifier into the `bundle-id` line of the project's `russet.toml`. If the file has
no such line, Russet opens it and asks you to set `bundle-id` yourself, then run again.

## When signing needs a change in your Apple account

To run on an iPhone, the app needs a certificate and a provisioning profile that lists the iPhone. When
Russet already has a matching profile, it uses it. When it would have to change your developer account
first, for example to register the iPhone or create a profile, Run stops with `perm.approvalRequired`. Such
changes need a person's approval, and this release of the IDE cannot grant it yet. See
`perm.approvalRequired` in the [Error reference](errors.md).

## Sign-in errors

Sign-in problems have their own codes, such as `gsa.badCredentials`, `gsa.badCode`, `gsa.identityRejected`
and `anisette.unsupported`. API key problems include `account.keyInvalid` and
`account.individualKeyNoProvisioning`. The [Error reference](errors.md) explains each one.
