# `hamstik auth forget`

Log out and forget a profile: remove its stored credential and its config entry (does not revoke the server-side PAT)

Log out and forget a profile: remove its stored credential and its config entry (does not revoke the server-side PAT).

The profile is named positionally: `hamstik auth forget NAME`, or run with no argument to forget the selected profile. Unlike `logout`, this runs even while HAMSTIK_TOKEN is set: forget is explicit about the profile being removed, not about the credential currently in use.

### `profile`

Profile to forget (defaults to the selected profile)

Value: `PROFILE`


Supports: `--json`, `--no-input`
