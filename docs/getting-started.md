# Your first Annex session

Annex is a community communication app you or someone you trust hosts. There is
no required central account, email address, or phone number. A server operator
controls admission, channels, agents, and federation.

## Join an existing community

1. [Install the app](installation.md) and launch it once to register invitation links.
2. Open the inviter's `https://monolithannex.com/invite/…` link. Check the server
   address and select **Open in Annex**. Allow the browser to open the app.
3. Create a new identity or import your backup. Proof generation can take time;
   leave the app open while it completes. Identity creation alone does not join
   an invite-only server.
4. Review the destination and choose **Join**. The server may reject an expired,
   revoked, or already-used invitation.
5. Select an available text channel and send a message. Channel permissions
   determine who can read, post, upload, and invite.

If you already have a server address, choose **Connect to a Server** and enter
it. A browser can use a server's web client when that operator has deployed one.
Browser storage and the desktop app do not automatically share your identity.

An invitation carries a bearer credential: anyone holding it may be able to use
it. Share it with the intended recipient; do not post it in public issue reports.

## Start a community

1. Create or import your identity in the desktop app.
2. Choose **Host a Server**. Annex starts an embedded server and creates its
   initial channel; you do not need to insert a channel into SQLite manually.
3. Review the server's settings, admission policy, channels, and permissions.
4. For remote members, configure reachable HTTPS and the correct public origin
   using the [deployment guide](deployment.md). Local hosting by itself is not
   internet hosting. The optional router registration flow is not a verified
   automatic tunnel service.
5. Generate an invitation in the app and test it from a separate device/network.
   A working local page alone does not establish remote reachability.

## Keep access to your identity

Use the app's identity backup/export flow and store the encrypted backup and its
password securely. Linking another device and social recovery are implemented,
but are not substitutes for testing your own backup. Clearing browser site data
or removing an app profile can remove locally stored keys. A server operator
cannot simply reset your cryptographic identity using an email address.

Do not share secret keys, recovery material, or backup passwords with an agent
or include them in a diagnostic report. Protect the device and its OS account.

## Use voice, video, and agents

Join an available voice channel and grant microphone permission. Camera and
screen sharing use the same in-process WebRTC media service. The operator must
provide working network connectivity, including TURN where necessary. Speech
transcription and agent speech are optional; human voice does not require a
Whisper model. Available speech profiles depend on installed providers.

Annex supports external AI participants but does not ship an LLM. An agent
runtime must implement the [connection protocol](protocol/agent-connection.md).
Review who is admitted before sharing private content: an authorized recipient,
including an agent, can read what you intentionally share with it.

## Know the boundaries

End-to-end encryption is a channel option, not a blanket property of all traffic.
Read [Encryption](ENCRYPTION.md) before choosing it. The first release has the
strongest native acceptance evidence on Linux. Full Windows/macOS invitation
and message journeys, updater installation, broader network conditions, and
all speech-provider combinations still need qualification. See
[Release readiness](RELEASE_READINESS.md) for evidence and remaining work.
