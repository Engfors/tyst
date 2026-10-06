# Security policy

## Supported versions

Only the latest release gets security fixes. Tyst is pre-1.0, so a fix ships as a new release rather than
a backport.

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub:
[Security › Report a vulnerability](https://github.com/Engfors/tyst/security/advisories/new).
Don't open a public issue, pull request or discussion for a security problem.

Include what you found, the version or commit, your OS, and steps or a proof of concept. Tyst has a single
maintainer: expect an acknowledgement within a week, and a fix or a decision soon after.
You'll be credited in the advisory unless you'd rather not be.

## Scope

Tyst runs entirely on your machine. Audio and transcripts never leave it, and the transcription core makes
no network calls. In scope, among others:

- Anything that sends audio, transcripts or keystrokes off the machine, or writes them where other local
  users can read them.
- The app's IPC commands, the dictation paste path and the meeting-detection prompt.
- Model download and verification (`tyst-cli models fetch`, pinned SHA-256 in `models/models.toml`).
- The update check, which only reads the GitHub releases API and never downloads or installs anything.
- The release pipeline: build workflows, AppImage packaging and release signing.

Out of scope: bugs in the models themselves (wrong transcriptions), and issues that need an attacker who
already controls your user account.

## Verifying releases

Every release has a `SHA256SUMS` file signed with the Tyst release key. The key's fingerprint is

```
6603 C039 2634 8BD9 8CE7  68DE E35B FE2B 59EC A014
```

Check it after importing `tyst-release-key.asc` (`gpg --fingerprint`) and before trusting the signature;
the [README](README.md#appimage-linux) has the full steps.
