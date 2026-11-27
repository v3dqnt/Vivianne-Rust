-- Vivianne accounts, migration 1. Paste the whole file into the Supabase SQL editor and run it once.
-- It runs as one transaction: if anything fails, nothing is changed.
-- Never edit this file after it has been run. A change goes in a new file (see supabase/README.md).
--
-- Supabase Auth (auth.users, auth.identities) holds who someone is: email, password or OAuth links, sessions.
-- This adds only what other people may see about them. Email addresses are never copied here.

begin;

-- Things no API client can reach. `private` is not an exposed schema.
create schema if not exists private;
revoke all on schema private from public, anon, authenticated;

create table if not exists private.schema_migrations (
    version    integer primary key,
    name       text not null,
    applied_at timestamptz not null default now()
);

do $$
begin
    if exists (select 1 from private.schema_migrations where version = 1) then
        raise exception 'migration 1 (accounts) has already been applied';
    end if;
end
$$;

-- Profiles -------------------------------------------------------------------------------------------------------

create table public.profiles (
    -- The same id as the auth user. Everything built later refers to this, never to an email or a handle.
    id           uuid primary key references auth.users (id) on delete cascade,
    -- What people type to find or invite someone. Lower case, so two cannot differ only in case.
    handle       text not null unique
                 check (handle ~ '^[a-z0-9][a-z0-9_-]{1,37}[a-z0-9]$'),
    display_name text
                 check (char_length(display_name) between 1 and 100 and display_name !~ '[[:cntrl:]]'),
    avatar_url   text
                 check (char_length(avatar_url) <= 500 and avatar_url ~ '^https://'),
    bio          text
                 check (char_length(bio) <= 500 and replace(bio, chr(10), '') !~ '[[:cntrl:]]'),
    -- Set by the platform only; people have no permission to write this column.
    status       text not null default 'active' check (status in ('active', 'suspended')),
    created_at   timestamptz not null default now(),
    updated_at   timestamptz not null default now()
);

comment on table public.profiles is 'What signed-in people may see about an account. No email or private data.';

-- Handles nobody may take.
create table private.reserved_handles (handle text primary key);
insert into private.reserved_handles (handle) values
    ('admin'), ('administrator'), ('api'), ('auth'), ('help'), ('moderator'), ('null'), ('root'),
    ('security'), ('settings'), ('support'), ('system'), ('undefined'), ('viv'), ('vivianne');

create function private.check_handle() returns trigger
language plpgsql security definer set search_path = ''
as $$
begin
    if exists (select 1 from private.reserved_handles r where r.handle = new.handle) then
        raise exception 'that handle is reserved' using errcode = 'check_violation';
    end if;
    return new;
end
$$;

create trigger profiles_check_handle
    before insert or update of handle on public.profiles
    for each row execute function private.check_handle();

-- Reusable by later tables that have `updated_at`.
create function private.touch_updated_at() returns trigger
language plpgsql set search_path = ''
as $$
begin
    new.updated_at := now();
    return new;
end
$$;

create trigger profiles_touch
    before update on public.profiles
    for each row execute function private.touch_updated_at();

-- A profile for every account ----------------------------------------------------------------------------------

-- Whatever a provider sends is trimmed to what the table accepts: a refused insert here would block sign-up.
create function private.handle_new_user() returns trigger
language plpgsql security definer set search_path = ''
as $$
declare
    wanted_name   text;
    wanted_avatar text;
begin
    wanted_name := coalesce(new.raw_user_meta_data ->> 'full_name', new.raw_user_meta_data ->> 'name');
    wanted_name := nullif(btrim(left(regexp_replace(wanted_name, '[[:cntrl:]]', '', 'g'), 100)), '');
    wanted_avatar := new.raw_user_meta_data ->> 'avatar_url';
    if wanted_avatar is null or wanted_avatar !~ '^https://' or char_length(wanted_avatar) > 500 then
        wanted_avatar := null;
    end if;

    insert into public.profiles (id, handle, display_name, avatar_url)
    values (
        new.id,
        -- A placeholder nobody else can have; the person picks their own afterwards.
        'u' || replace(new.id::text, '-', ''),
        wanted_name,
        wanted_avatar
    )
    on conflict (id) do nothing;
    return new;
end
$$;

create trigger on_auth_user_created
    after insert on auth.users
    for each row execute function private.handle_new_user();

-- Accounts that existed before this migration.
insert into public.profiles (id, handle)
select u.id, 'u' || replace(u.id::text, '-', '')
from auth.users u
on conflict (id) do nothing;

-- Who may do what ----------------------------------------------------------------------------------------------

alter table public.profiles enable row level security;

-- Start from nothing, then give back exactly what is needed.
revoke all on public.profiles from public, anon, authenticated;
grant select on public.profiles to authenticated;
grant update (handle, display_name, avatar_url, bio) on public.profiles to authenticated;
grant all on public.profiles to service_role;

-- Signed-in people see active profiles, and always their own. Signed-out visitors see none.
create policy profiles_read on public.profiles
    for select to authenticated
    using (status = 'active' or id = (select auth.uid()));

create policy profiles_edit_own on public.profiles
    for update to authenticated
    using (id = (select auth.uid()) and status = 'active')
    with check (id = (select auth.uid()));

-- No insert or delete policy: a profile is made by the sign-up trigger and goes when the auth user is deleted.

-- Linked sign-ins ----------------------------------------------------------------------------------------------

-- The caller's own linked providers, as the provider verified them (GitHub's numeric id and login). Read from
-- Supabase Auth each time, so nothing here can be typed in or go stale.
create function public.my_identities()
returns table (provider text, provider_user_id text, login text, linked_at timestamptz)
language sql stable security definer set search_path = ''
as $$
    select i.provider, i.provider_id, i.identity_data ->> 'user_name', i.created_at
    from auth.identities i
    where i.user_id = (select auth.uid())
$$;

revoke all on function public.my_identities() from public, anon;
grant execute on function public.my_identities() to authenticated, service_role;

insert into private.schema_migrations (version, name) values (1, 'accounts');

commit;
