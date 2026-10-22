# User accounts

Kahawai Server starts with a single pairing token (see Installation). Once
the household grows — or you just want per-person sign-in — create user
accounts. The first admin retires the pairing token; from then on everyone
signs in with a username and password.

## Creating the first admin

1. Open the server's home page (`http://<server>:8080/`) in a browser.
2. Fill in **Create the first admin account** (username, password, the
   pairing token). Submit.
3. The pairing token is deleted from the keychain and retired: the setup
   form and `POST /api/setup` disappear (404) from then on.

Admins can create further users (and more admins) via the API:

```
POST /api/users          { "username": "...", "password": "...", "is_admin": false }
GET  /api/users
DELETE /api/users/{id}
POST /api/users/{id}/password   { "new_password": "..." }
```

All of these require an admin session (`Authorization: Bearer <token>`).

## Signing in (players)

Kahawai Player prompts automatically: enter the server URL, username, and
password. The session token is kept in the OS keychain. A 401 becomes
"Session expired — log in again".

## Sessions

- Sessions are random 256-bit tokens; only their SHA-256 hash is stored.
- Sessions expire 30 days after creation (absolute — activity doesn't
  extend them).
- **Log out** deletes the session. Changing your password revokes **all**
  your sessions (other devices are signed out).
- Login is throttled: 5 attempts per minute per IP with exponential
  backoff. Failures always return the same 401, whether the username
  exists or not.

## Credential recovery (no email)

There is no password-reset email — the server is yours, on your Mac, so
recovery is physical access to the machine:

1. **You still have an admin session somewhere** (a signed-in player):
   use it to set a new password (`POST /api/users/{id}/password`).
2. **No admin session, but you have the Mac:** stop the server and delete
   its `users` rows, then restart — the server is back to pairing-token
   mode and you can create the first admin again:
   ```
   sqlite3 /path/to/music.db "DELETE FROM sessions; DELETE FROM users;"
   ```
   (The database path is shown in the server app's Settings tab.)
3. **Locked out of the Mac too:** that's outside Kahawai's reach — macOS
   account recovery applies.

Deleting the users table never touches the music library: users and
sessions live in their own tables (migration 022), and dropping them only
resets access control.
