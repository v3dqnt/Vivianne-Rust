# supabase

Vivianne's accounts live in a Supabase project: Supabase Auth signs people in, and the SQL here adds what Vivianne
needs on top. `migrations/` holds the schema, one numbered file per change.

## Running it

Dashboard, then SQL Editor, then New query: paste a migration file and run it, in number order, once each. A file is
one transaction and refuses to run twice. `select * from private.schema_migrations order by version;` shows what a
database already has.

## Building on it

- Never edit a migration that has been run. Add `migrations/000N_name.sql` with the same shape: `begin;`, the check
  that version N is not applied, the change, the insert into `private.schema_migrations`, `commit;`.
- Refer to a person by `public.profiles.id` (the auth user's id), never by email or handle.
- Every new table: `enable row level security`, `revoke all` from `anon` and `authenticated`, then grant only what is
  needed, with a policy for each. A table without a policy is closed to clients.
- Anything clients must not reach goes in the `private` schema. Do not add `private` to the exposed schemas.
- A `security definer` function sets `search_path = ''`, names every schema, and has `execute` revoked from `public`
  unless it is a trigger.
- Roles, memberships and project permissions are enforced by `viv-hub` (`crates/projects`), not by client-side
  queries. Project tables, when they move here from SQLite, are written by the service only.

## Keys

- **Safe in the app:** the project URL and the publishable (older name: `anon`) key. Row level security is what
  protects the data.
- **Never in the repository, the app, or a chat:** the secret (`service_role`) key, the database password and any
  signing secret. They bypass row level security. They go in the server's environment only.
