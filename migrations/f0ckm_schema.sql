-- Dumped from database version 17.9 (Debian 17.9-1.pgdg13+1)
-- Dumped by pg_dump version 17.9 (Debian 17.9-1.pgdg13+1)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET transaction_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', 'public', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

DROP PUBLICATION IF EXISTS alltables;
ALTER TABLE IF EXISTS ONLY public.webauthn_credentials DROP CONSTRAINT IF EXISTS webauthn_credentials_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_warnings DROP CONSTRAINT IF EXISTS user_warnings_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_warnings DROP CONSTRAINT IF EXISTS user_warnings_admin_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_video_views DROP CONSTRAINT IF EXISTS user_video_views_video_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_video_views DROP CONSTRAINT IF EXISTS user_video_views_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_sessions DROP CONSTRAINT IF EXISTS user_sessions_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_pubkeys DROP CONSTRAINT IF EXISTS user_pubkeys_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_options DROP CONSTRAINT IF EXISTS user_options_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_options DROP CONSTRAINT IF EXISTS user_options_avatar_fkey;
ALTER TABLE IF EXISTS ONLY public.user_ips DROP CONSTRAINT IF EXISTS user_ips_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_halls DROP CONSTRAINT IF EXISTS user_halls_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_halls_assign DROP CONSTRAINT IF EXISTS user_halls_assign_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_halls_assign DROP CONSTRAINT IF EXISTS user_halls_assign_item_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_halls_assign DROP CONSTRAINT IF EXISTS user_halls_assign_hall_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_emoji_favorites DROP CONSTRAINT IF EXISTS user_emoji_favorites_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_dm_keyvault DROP CONSTRAINT IF EXISTS user_dm_keyvault_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_conversation_states DROP CONSTRAINT IF EXISTS user_conversation_states_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_conversation_states DROP CONSTRAINT IF EXISTS user_conversation_states_other_id_fkey;
ALTER TABLE IF EXISTS ONLY public.user_api_keys DROP CONSTRAINT IF EXISTS user_api_keys_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.tags_assign DROP CONSTRAINT IF EXISTS tags_assign_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.tags_assign DROP CONSTRAINT IF EXISTS tags_assign_tag_id_fkey;
ALTER TABLE IF EXISTS ONLY public.tags_assign DROP CONSTRAINT IF EXISTS tags_assign_item_id_fkey;
ALTER TABLE IF EXISTS ONLY public.reports DROP CONSTRAINT IF EXISTS reports_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.reports DROP CONSTRAINT IF EXISTS reports_resolved_by_fkey;
ALTER TABLE IF EXISTS ONLY public.reports DROP CONSTRAINT IF EXISTS reports_reporter_id_fkey;
ALTER TABLE IF EXISTS ONLY public.reports DROP CONSTRAINT IF EXISTS reports_item_id_fkey;
ALTER TABLE IF EXISTS ONLY public.reports DROP CONSTRAINT IF EXISTS reports_comment_id_fkey;
ALTER TABLE IF EXISTS ONLY public.private_messages DROP CONSTRAINT IF EXISTS private_messages_sender_id_fkey;
ALTER TABLE IF EXISTS ONLY public.private_messages DROP CONSTRAINT IF EXISTS private_messages_recipient_id_fkey;
ALTER TABLE IF EXISTS ONLY public.notifications DROP CONSTRAINT IF EXISTS notifications_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.notifications DROP CONSTRAINT IF EXISTS notifications_item_id_fkey;
ALTER TABLE IF EXISTS ONLY public.link_token DROP CONSTRAINT IF EXISTS link_token_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.invite_tokens DROP CONSTRAINT IF EXISTS invite_tokens_used_by_fkey;
ALTER TABLE IF EXISTS ONLY public.invite_tokens DROP CONSTRAINT IF EXISTS invite_tokens_created_by_fkey;
ALTER TABLE IF EXISTS ONLY public.halls_assign DROP CONSTRAINT IF EXISTS halls_assign_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.halls_assign DROP CONSTRAINT IF EXISTS halls_assign_item_id_fkey;
ALTER TABLE IF EXISTS ONLY public.halls_assign DROP CONSTRAINT IF EXISTS halls_assign_hall_id_fkey;
ALTER TABLE IF EXISTS ONLY public.global_chat DROP CONSTRAINT IF EXISTS global_chat_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.favorites DROP CONSTRAINT IF EXISTS favorites_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.favorites DROP CONSTRAINT IF EXISTS favorites_item_id_fkey;
ALTER TABLE IF EXISTS ONLY public.dm_attachments DROP CONSTRAINT IF EXISTS dm_attachments_sender_id_fkey;
ALTER TABLE IF EXISTS ONLY public.dm_attachments DROP CONSTRAINT IF EXISTS dm_attachments_recipient_id_fkey;
ALTER TABLE IF EXISTS ONLY public.discord_queue DROP CONSTRAINT IF EXISTS discord_queue_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.discord_queue DROP CONSTRAINT IF EXISTS discord_queue_item_id_fkey;
ALTER TABLE IF EXISTS ONLY public.custom_emojis DROP CONSTRAINT IF EXISTS custom_emojis_pack_id_fkey;
ALTER TABLE IF EXISTS ONLY public.comments DROP CONSTRAINT IF EXISTS comments_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.comments DROP CONSTRAINT IF EXISTS comments_parent_id_fkey;
ALTER TABLE IF EXISTS ONLY public.comments DROP CONSTRAINT IF EXISTS comments_item_id_fkey;
ALTER TABLE IF EXISTS ONLY public.comment_subscriptions DROP CONSTRAINT IF EXISTS comment_subscriptions_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.comment_subscriptions DROP CONSTRAINT IF EXISTS comment_subscriptions_item_id_fkey;
ALTER TABLE IF EXISTS ONLY public.comment_polls DROP CONSTRAINT IF EXISTS comment_polls_comment_id_fkey;
ALTER TABLE IF EXISTS ONLY public.comment_poll_votes DROP CONSTRAINT IF EXISTS comment_poll_votes_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.comment_poll_votes DROP CONSTRAINT IF EXISTS comment_poll_votes_poll_id_fkey;
ALTER TABLE IF EXISTS ONLY public.comment_poll_votes DROP CONSTRAINT IF EXISTS comment_poll_votes_option_id_fkey;
ALTER TABLE IF EXISTS ONLY public.comment_poll_options DROP CONSTRAINT IF EXISTS comment_poll_options_poll_id_fkey;
ALTER TABLE IF EXISTS ONLY public.comment_files DROP CONSTRAINT IF EXISTS comment_files_user_id_fkey;
ALTER TABLE IF EXISTS ONLY public.comment_files DROP CONSTRAINT IF EXISTS comment_files_comment_id_fkey;
DROP TRIGGER IF EXISTS trigger_notify_private_message ON public.private_messages;
DROP TRIGGER IF EXISTS trigger_notify_notification ON public.notifications;
DROP TRIGGER IF EXISTS trigger_notify_activity ON public.comments;

DROP TRIGGER IF EXISTS tags_bu ON public.tags;
DROP TRIGGER IF EXISTS tags_bi ON public.tags;
DROP TRIGGER IF EXISTS tags_assign_ad ON public.tags_assign;
DROP TRIGGER IF EXISTS global_chat_notify ON public.global_chat;
DROP INDEX IF EXISTS public.tags_assign_tag_id_idx;
DROP INDEX IF EXISTS public.items_phash;
DROP INDEX IF EXISTS public.idx_webauthn_credentials_user_id;
DROP INDEX IF EXISTS public.idx_user_warnings_user_status;
DROP INDEX IF EXISTS public.idx_user_last_seen;
DROP INDEX IF EXISTS public.idx_user_ips_user_id;
DROP INDEX IF EXISTS public.idx_user_halls_user_id;
DROP INDEX IF EXISTS public.idx_user_halls_assign_item;
DROP INDEX IF EXISTS public.idx_user_halls_assign_hall;
DROP INDEX IF EXISTS public.idx_user_emoji_favorites_user_id;
DROP INDEX IF EXISTS public.idx_user_alias_userid;
DROP INDEX IF EXISTS public.idx_user_alias_type;
DROP INDEX IF EXISTS public.idx_user_alias_alias;
DROP INDEX IF EXISTS public.idx_ucs_user_hidden;
DROP INDEX IF EXISTS public.idx_reports_status;
DROP INDEX IF EXISTS public.idx_pm_sender;
DROP INDEX IF EXISTS public.idx_pm_recipient;
DROP INDEX IF EXISTS public.idx_pm_conversation;
DROP INDEX IF EXISTS public.idx_notifications_user_id;
DROP INDEX IF EXISTS public.idx_notifications_unread;
DROP INDEX IF EXISTS public.idx_login_attempts_username_time;
DROP INDEX IF EXISTS public.idx_login_attempts_ip_time;

DROP INDEX IF EXISTS public.idx_items_username_lower;
DROP INDEX IF EXISTS public.idx_items_username;
DROP INDEX IF EXISTS public.idx_items_pinned_id;
DROP INDEX IF EXISTS public.idx_items_is_purged;
DROP INDEX IF EXISTS public.idx_items_is_deleted;
DROP INDEX IF EXISTS public.idx_items_expires_at;
DROP INDEX IF EXISTS public.idx_items_active_id;
DROP INDEX IF EXISTS public.idx_invite_tokens_used_at;
DROP INDEX IF EXISTS public.idx_invite_tokens_created_by;
DROP INDEX IF EXISTS public.idx_global_chat_created_at;
DROP INDEX IF EXISTS public.idx_dm_att_sender;
DROP INDEX IF EXISTS public.idx_dm_att_recipient;
DROP INDEX IF EXISTS public.idx_dm_att_expires;
DROP INDEX IF EXISTS public.idx_discord_queue_sent;
DROP INDEX IF EXISTS public.idx_comments_user_id;
DROP INDEX IF EXISTS public.idx_comments_item_id;
DROP INDEX IF EXISTS public.idx_comments_is_pinned;
DROP INDEX IF EXISTS public.idx_comment_poll_votes_user;
DROP INDEX IF EXISTS public.idx_comment_poll_votes_poll;
DROP INDEX IF EXISTS public.idx_comment_poll_options_poll;
DROP INDEX IF EXISTS public.idx_comment_files_comment_id;
DROP INDEX IF EXISTS public.idx_comment_files_checksum;
DROP INDEX IF EXISTS public.idx_audit_log_user_id;
DROP INDEX IF EXISTS public.idx_audit_log_created_at;
DROP INDEX IF EXISTS public.idx_audit_log_action;
DROP INDEX IF EXISTS public.custom_emojis_pack_id_idx;
ALTER TABLE IF EXISTS ONLY public.wordfilter DROP CONSTRAINT IF EXISTS wordfilter_word_key;
ALTER TABLE IF EXISTS ONLY public.wordfilter DROP CONSTRAINT IF EXISTS wordfilter_pkey;
ALTER TABLE IF EXISTS ONLY public.webauthn_credentials DROP CONSTRAINT IF EXISTS webauthn_credentials_pkey;
ALTER TABLE IF EXISTS ONLY public.webauthn_credentials DROP CONSTRAINT IF EXISTS webauthn_credentials_cred_id_key;
ALTER TABLE IF EXISTS ONLY public.user_warnings DROP CONSTRAINT IF EXISTS user_warnings_pkey;
ALTER TABLE IF EXISTS ONLY public.user_video_views DROP CONSTRAINT IF EXISTS user_video_views_pkey;
ALTER TABLE IF EXISTS ONLY public.user_pubkeys DROP CONSTRAINT IF EXISTS user_pubkeys_pkey;
ALTER TABLE IF EXISTS ONLY public.user_options DROP CONSTRAINT IF EXISTS user_options_user_id;
ALTER TABLE IF EXISTS ONLY public."user" DROP CONSTRAINT IF EXISTS user_name_unique;
ALTER TABLE IF EXISTS ONLY public."user" DROP CONSTRAINT IF EXISTS user_login_unique;
ALTER TABLE IF EXISTS ONLY public.user_ips DROP CONSTRAINT IF EXISTS user_ips_user_id_ip_key;
ALTER TABLE IF EXISTS ONLY public.user_ips DROP CONSTRAINT IF EXISTS user_ips_pkey;
ALTER TABLE IF EXISTS ONLY public.user_halls DROP CONSTRAINT IF EXISTS user_halls_user_id_slug_key;
ALTER TABLE IF EXISTS ONLY public.user_halls DROP CONSTRAINT IF EXISTS user_halls_pkey;
ALTER TABLE IF EXISTS ONLY public.user_halls_assign DROP CONSTRAINT IF EXISTS user_halls_assign_pkey;
ALTER TABLE IF EXISTS ONLY public.user_emoji_favorites DROP CONSTRAINT IF EXISTS user_emoji_favorites_pkey;
ALTER TABLE IF EXISTS ONLY public."user" DROP CONSTRAINT IF EXISTS user_email_unique;
ALTER TABLE IF EXISTS ONLY public.user_dm_keyvault DROP CONSTRAINT IF EXISTS user_dm_keyvault_pkey;
ALTER TABLE IF EXISTS ONLY public.user_conversation_states DROP CONSTRAINT IF EXISTS user_conversation_states_pkey;
ALTER TABLE IF EXISTS ONLY public.user_api_keys DROP CONSTRAINT IF EXISTS user_api_keys_pkey;
ALTER TABLE IF EXISTS ONLY public.user_api_keys DROP CONSTRAINT IF EXISTS user_api_keys_api_key_key;
ALTER TABLE IF EXISTS ONLY public.tags DROP CONSTRAINT IF EXISTS tags_id;
ALTER TABLE IF EXISTS ONLY public.tags_assign DROP CONSTRAINT IF EXISTS tags_assign_item_id_tag_id_unique;
ALTER TABLE IF EXISTS ONLY public.tags_assign DROP CONSTRAINT IF EXISTS tags_assign_item_id_tag_id_primary;
ALTER TABLE IF EXISTS ONLY public.sticker_packs DROP CONSTRAINT IF EXISTS sticker_packs_tg_name_key;
ALTER TABLE IF EXISTS ONLY public.sticker_packs DROP CONSTRAINT IF EXISTS sticker_packs_pkey;
ALTER TABLE IF EXISTS ONLY public.site_settings DROP CONSTRAINT IF EXISTS site_settings_pkey;
ALTER TABLE IF EXISTS ONLY public.reports DROP CONSTRAINT IF EXISTS reports_pkey;
ALTER TABLE IF EXISTS ONLY public.private_messages DROP CONSTRAINT IF EXISTS private_messages_pkey;
ALTER TABLE IF EXISTS ONLY public.notifications DROP CONSTRAINT IF EXISTS notifications_pkey;
ALTER TABLE IF EXISTS ONLY public.meta_cache DROP CONSTRAINT IF EXISTS meta_cache_pkey;
ALTER TABLE IF EXISTS ONLY public.meme_templates DROP CONSTRAINT IF EXISTS meme_templates_template_id_key;
ALTER TABLE IF EXISTS ONLY public.meme_templates DROP CONSTRAINT IF EXISTS meme_templates_pkey;
ALTER TABLE IF EXISTS ONLY public.login_attempts DROP CONSTRAINT IF EXISTS login_attempts_pkey;
ALTER TABLE IF EXISTS ONLY public.link_token DROP CONSTRAINT IF EXISTS link_token_token_key;
ALTER TABLE IF EXISTS ONLY public.link_token DROP CONSTRAINT IF EXISTS link_token_pkey;
ALTER TABLE IF EXISTS ONLY public.items DROP CONSTRAINT IF EXISTS items_slug_key;
ALTER TABLE IF EXISTS ONLY public.items DROP CONSTRAINT IF EXISTS items_checksum;
ALTER TABLE IF EXISTS ONLY public.invite_tokens DROP CONSTRAINT IF EXISTS invite_tokens_token_key;
ALTER TABLE IF EXISTS ONLY public.invite_tokens DROP CONSTRAINT IF EXISTS invite_tokens_pkey;
ALTER TABLE IF EXISTS ONLY public.user_sessions DROP CONSTRAINT IF EXISTS idx_16572_primary;
ALTER TABLE IF EXISTS ONLY public.user_options DROP CONSTRAINT IF EXISTS idx_16567_user_id;
ALTER TABLE IF EXISTS ONLY public."user" DROP CONSTRAINT IF EXISTS idx_16554_primary;
ALTER TABLE IF EXISTS ONLY public.items DROP CONSTRAINT IF EXISTS idx_16526_primary;
ALTER TABLE IF EXISTS ONLY public.favorites DROP CONSTRAINT IF EXISTS idx_16521_primary;
ALTER TABLE IF EXISTS ONLY public.halls DROP CONSTRAINT IF EXISTS halls_slug_key;
ALTER TABLE IF EXISTS ONLY public.halls DROP CONSTRAINT IF EXISTS halls_pkey;
ALTER TABLE IF EXISTS ONLY public.halls_assign DROP CONSTRAINT IF EXISTS halls_assign_pkey;
ALTER TABLE IF EXISTS ONLY public.global_chat_settings DROP CONSTRAINT IF EXISTS global_chat_settings_pkey;
ALTER TABLE IF EXISTS ONLY public.global_chat DROP CONSTRAINT IF EXISTS global_chat_pkey;
ALTER TABLE IF EXISTS ONLY public.dm_attachments DROP CONSTRAINT IF EXISTS dm_attachments_pkey;
ALTER TABLE IF EXISTS ONLY public.discord_queue DROP CONSTRAINT IF EXISTS discord_queue_pkey;
ALTER TABLE IF EXISTS ONLY public.custom_emojis DROP CONSTRAINT IF EXISTS custom_emojis_pkey;
ALTER TABLE IF EXISTS ONLY public.custom_emojis DROP CONSTRAINT IF EXISTS custom_emojis_name_key;
ALTER TABLE IF EXISTS ONLY public.comments DROP CONSTRAINT IF EXISTS comments_pkey;
ALTER TABLE IF EXISTS ONLY public.comment_subscriptions DROP CONSTRAINT IF EXISTS comment_subscriptions_pkey;
ALTER TABLE IF EXISTS ONLY public.comment_polls DROP CONSTRAINT IF EXISTS comment_polls_pkey;
ALTER TABLE IF EXISTS ONLY public.comment_polls DROP CONSTRAINT IF EXISTS comment_polls_comment_id_key;
ALTER TABLE IF EXISTS ONLY public.comment_poll_votes DROP CONSTRAINT IF EXISTS comment_poll_votes_poll_id_user_id_key;
ALTER TABLE IF EXISTS ONLY public.comment_poll_votes DROP CONSTRAINT IF EXISTS comment_poll_votes_pkey;
ALTER TABLE IF EXISTS ONLY public.comment_poll_options DROP CONSTRAINT IF EXISTS comment_poll_options_pkey;
ALTER TABLE IF EXISTS ONLY public.comment_files DROP CONSTRAINT IF EXISTS comment_files_pkey;
ALTER TABLE IF EXISTS ONLY public.audit_log DROP CONSTRAINT IF EXISTS audit_log_pkey;
ALTER TABLE IF EXISTS public.wordfilter ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.webauthn_credentials ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.user_warnings ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.user_ips ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.user_halls ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.sticker_packs ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.reports ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.private_messages ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.notifications ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.meme_templates ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.login_attempts ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.link_token ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.invite_tokens ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.halls ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.global_chat ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.dm_attachments ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.discord_queue ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.custom_emojis ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.comments ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.comment_polls ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.comment_poll_votes ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.comment_poll_options ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.comment_files ALTER COLUMN id DROP DEFAULT;
ALTER TABLE IF EXISTS public.audit_log ALTER COLUMN id DROP DEFAULT;
DROP SEQUENCE IF EXISTS public.wordfilter_id_seq;
DROP TABLE IF EXISTS public.wordfilter;
DROP SEQUENCE IF EXISTS public.webauthn_credentials_id_seq;
DROP TABLE IF EXISTS public.webauthn_credentials;
DROP SEQUENCE IF EXISTS public.user_warnings_id_seq;
DROP TABLE IF EXISTS public.user_warnings;
DROP TABLE IF EXISTS public.user_video_views;
DROP TABLE IF EXISTS public.user_sessions;
DROP SEQUENCE IF EXISTS public.user_sessions_id_seq;
DROP TABLE IF EXISTS public.user_pubkeys;
DROP TABLE IF EXISTS public.user_options;
DROP SEQUENCE IF EXISTS public.user_ips_id_seq;
DROP TABLE IF EXISTS public.user_ips;
DROP SEQUENCE IF EXISTS public.user_halls_id_seq;
DROP TABLE IF EXISTS public.user_halls_assign;
DROP TABLE IF EXISTS public.user_halls;
DROP TABLE IF EXISTS public.user_emoji_favorites;
DROP TABLE IF EXISTS public.user_dm_keyvault;
DROP TABLE IF EXISTS public.user_conversation_states;
DROP TABLE IF EXISTS public.user_api_keys;
DROP TABLE IF EXISTS public.user_alias;
DROP TABLE IF EXISTS public."user";
DROP SEQUENCE IF EXISTS public.user_id_seq;
DROP TABLE IF EXISTS public.tags;
DROP SEQUENCE IF EXISTS public.tags_id_seq;
DROP SEQUENCE IF EXISTS public.sticker_packs_id_seq;
DROP TABLE IF EXISTS public.sticker_packs;
DROP TABLE IF EXISTS public.site_settings;
DROP SEQUENCE IF EXISTS public.reports_id_seq;
DROP TABLE IF EXISTS public.reports;
DROP SEQUENCE IF EXISTS public.private_messages_id_seq;
DROP TABLE IF EXISTS public.private_messages;
DROP SEQUENCE IF EXISTS public.notifications_id_seq;
DROP TABLE IF EXISTS public.notifications;
DROP TABLE IF EXISTS public.meta_cache;
DROP SEQUENCE IF EXISTS public.meme_templates_id_seq;
DROP TABLE IF EXISTS public.meme_templates;
DROP SEQUENCE IF EXISTS public.login_attempts_id_seq;
DROP TABLE IF EXISTS public.login_attempts;
DROP SEQUENCE IF EXISTS public.link_token_id_seq;
DROP TABLE IF EXISTS public.link_token;
DROP TABLE IF EXISTS public.tags_assign;
DROP TABLE IF EXISTS public.items;
DROP SEQUENCE IF EXISTS public.items_id_seq;
DROP SEQUENCE IF EXISTS public.invite_tokens_id_seq;
DROP TABLE IF EXISTS public.invite_tokens;
DROP SEQUENCE IF EXISTS public.halls_id_seq;
DROP TABLE IF EXISTS public.halls_assign;
DROP TABLE IF EXISTS public.halls;
DROP TABLE IF EXISTS public.global_chat_settings;
DROP SEQUENCE IF EXISTS public.global_chat_id_seq;
DROP TABLE IF EXISTS public.global_chat;
DROP TABLE IF EXISTS public.favorites;
DROP SEQUENCE IF EXISTS public.dm_attachments_id_seq;
DROP TABLE IF EXISTS public.dm_attachments;
DROP SEQUENCE IF EXISTS public.discord_queue_id_seq;
DROP TABLE IF EXISTS public.discord_queue;
DROP SEQUENCE IF EXISTS public.custom_emojis_id_seq;
DROP TABLE IF EXISTS public.custom_emojis;
DROP SEQUENCE IF EXISTS public.comments_id_seq;
DROP TABLE IF EXISTS public.comments;
DROP TABLE IF EXISTS public.comment_subscriptions;
DROP SEQUENCE IF EXISTS public.comment_polls_id_seq;
DROP TABLE IF EXISTS public.comment_polls;
DROP SEQUENCE IF EXISTS public.comment_poll_votes_id_seq;
DROP TABLE IF EXISTS public.comment_poll_votes;
DROP SEQUENCE IF EXISTS public.comment_poll_options_id_seq;
DROP TABLE IF EXISTS public.comment_poll_options;
DROP SEQUENCE IF EXISTS public.comment_files_id_seq;
DROP TABLE IF EXISTS public.comment_files;
DROP SEQUENCE IF EXISTS public.audit_log_id_seq;
DROP TABLE IF EXISTS public.audit_log;

DROP FUNCTION IF EXISTS public.unaccent_text(text);

DROP FUNCTION IF EXISTS public.slugify(v text);
DROP FUNCTION IF EXISTS public.notify_private_message();
DROP FUNCTION IF EXISTS public.notify_notification();
DROP FUNCTION IF EXISTS public.notify_global_chat();
DROP FUNCTION IF EXISTS public.notify_activity();
DROP FUNCTION IF EXISTS public.fill_normalized();
DROP FUNCTION IF EXISTS public.delete_unused_tags();
DROP EXTENSION IF EXISTS unaccent;
-- *not* dropping schema, since initdb creates it
--
-- Name: public; Type: SCHEMA; Schema: -; Owner: -
--

-- *not* creating schema, since initdb creates it


--
-- Name: SCHEMA public; Type: COMMENT; Schema: -; Owner: -
--

COMMENT ON SCHEMA public IS '';


--
-- Name: unaccent; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS unaccent WITH SCHEMA public;


--
-- Name: EXTENSION unaccent; Type: COMMENT; Schema: -; Owner: -
--

COMMENT ON EXTENSION unaccent IS 'text search dictionary that removes accents';


--
-- Name: delete_unused_tags(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.delete_unused_tags() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
begin
  delete from tags
  where
    tags.id not in (select tag_id from tags_assign) and
    tags.id = OLD.tag_id and
    tags.tag != 'sfw' and
    tags.tag != 'nsfw';
  return OLD;
end $$;


--
-- Name: fill_normalized(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.fill_normalized() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
begin
  NEW.normalized = slugify(NEW.tag);
  return NEW;
end$$;


--
-- Name: notify_activity(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.notify_activity() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
 DECLARE
     payload JSON;
 BEGIN
     SELECT json_build_object(
         'id', NEW.id,
         'user_id', NEW.user_id,
         'item_id', NEW.item_id,
         'parent_id', NEW.parent_id,
         'body', LEFT(NEW.content, 2000), -- Truncate to prevent payload string too long error
         'created_at', NEW.created_at
     ) INTO payload;
     
     PERFORM pg_notify('activity', payload::text);
     RETURN NEW;
 END;
 $$;


--
-- Name: notify_global_chat(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.notify_global_chat() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    PERFORM pg_notify('global_chat', row_to_json(NEW)::text);
    RETURN NEW;
END;
$$;


--
-- Name: notify_notification(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.notify_notification() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    payload JSON;
BEGIN
    -- Only notify on INSERT or when a notification transitions to is_read = true
    IF (TG_OP = 'INSERT') OR (TG_OP = 'UPDATE' AND OLD.is_read = false AND NEW.is_read = true) THEN
        SELECT json_build_object(
            'id', NEW.id,
            'user_id', NEW.user_id,
            'type', NEW.type,
            'item_id', NEW.item_id,
            'is_read', NEW.is_read,
            'created_at', NEW.created_at
        ) INTO payload;
        
        PERFORM pg_notify('notifications', payload::text);
    END IF;
    RETURN NEW;
END;
$$;


--
-- Name: notify_private_message(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.notify_private_message() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    PERFORM pg_notify('private_message', json_build_object(
        'id',           NEW.id,
        'sender_id',    NEW.sender_id,
        'recipient_id', NEW.recipient_id,
        'created_at',   NEW.created_at
    )::text);
    RETURN NEW;
END;
$$;


--
-- Name: slugify(text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.slugify(v text) RETURNS text
    LANGUAGE plpgsql
    AS $$
BEGIN
  RETURN trim(BOTH '-' FROM regexp_replace(lower(unaccent(trim(v))), '[\u0000-\u002f \u003a-\u0040\u005b-\u0060\u007b-\u00bf]+', '', 'gi'));
END;
$$;



--
-- Name: unaccent_text(text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.unaccent_text(text) RETURNS text
    LANGUAGE sql IMMUTABLE COST 1
    AS $_$
  -- unaccent is STABLE, but the indexes must use IMMUTABLE functions.
  -- comment this line out when calling pg_dump.
  SELECT unaccent($1);

  -- Uncomment this line when calling pg_dump.
  --SELECT ''::text;
$_$;



SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: audit_log; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.audit_log (
    id integer NOT NULL,
    user_id integer NOT NULL,
    action character varying(50) NOT NULL,
    target_type character varying(50),
    target_id character varying(50),
    details jsonb,
    created_at timestamp with time zone DEFAULT now()
);


--
-- Name: audit_log_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.audit_log_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: audit_log_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.audit_log_id_seq OWNED BY public.audit_log.id;


--
-- Name: comment_files; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.comment_files (
    id integer NOT NULL,
    comment_id integer,
    user_id integer NOT NULL,
    dest character varying(60) NOT NULL,
    mime character varying(100) NOT NULL,
    size integer NOT NULL,
    checksum character varying(255) NOT NULL,
    phash text,
    original_filename text,
    created_at timestamp with time zone DEFAULT now()
);


--
-- Name: comment_files_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.comment_files_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: comment_files_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.comment_files_id_seq OWNED BY public.comment_files.id;


--
-- Name: comment_poll_options; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.comment_poll_options (
    id integer NOT NULL,
    poll_id integer NOT NULL,
    text text NOT NULL,
    sort_order smallint DEFAULT 0 NOT NULL
);


--
-- Name: comment_poll_options_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.comment_poll_options_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: comment_poll_options_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.comment_poll_options_id_seq OWNED BY public.comment_poll_options.id;


--
-- Name: comment_poll_votes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.comment_poll_votes (
    id integer NOT NULL,
    poll_id integer NOT NULL,
    option_id integer NOT NULL,
    user_id integer NOT NULL,
    created_at timestamp with time zone DEFAULT now()
);


--
-- Name: comment_poll_votes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.comment_poll_votes_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: comment_poll_votes_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.comment_poll_votes_id_seq OWNED BY public.comment_poll_votes.id;


--
-- Name: comment_polls; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.comment_polls (
    id integer NOT NULL,
    comment_id integer NOT NULL,
    question text NOT NULL,
    is_anonymous boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now(),
    expires_at timestamp with time zone
);


--
-- Name: comment_polls_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.comment_polls_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: comment_polls_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.comment_polls_id_seq OWNED BY public.comment_polls.id;


--
-- Name: comment_subscriptions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.comment_subscriptions (
    user_id integer NOT NULL,
    item_id integer NOT NULL,
    created_at timestamp with time zone DEFAULT now(),
    is_subscribed boolean DEFAULT true
);


--
-- Name: comments; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.comments (
    id integer NOT NULL,
    item_id integer NOT NULL,
    user_id integer NOT NULL,
    parent_id integer,
    content text NOT NULL,
    is_deleted boolean DEFAULT false,
    created_at timestamp with time zone DEFAULT now(),
    updated_at timestamp with time zone,
    vote_score integer DEFAULT 0,
    is_pinned boolean DEFAULT false,
    video_time numeric(10,3)
);


--
-- Name: comments_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.comments_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: comments_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.comments_id_seq OWNED BY public.comments.id;


--
-- Name: custom_emojis; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.custom_emojis (
    id integer NOT NULL,
    name text NOT NULL,
    url text NOT NULL,
    created_at timestamp with time zone DEFAULT now(),
    pack_id integer
);


--
-- Name: custom_emojis_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.custom_emojis_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: custom_emojis_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.custom_emojis_id_seq OWNED BY public.custom_emojis.id;


--
-- Name: discord_queue; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.discord_queue (
    id integer NOT NULL,
    user_id integer NOT NULL,
    item_id integer NOT NULL,
    type character varying(50) DEFAULT 'approve'::character varying NOT NULL,
    sent boolean DEFAULT false,
    created_at timestamp without time zone DEFAULT now()
);


--
-- Name: discord_queue_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.discord_queue_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: discord_queue_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.discord_queue_id_seq OWNED BY public.discord_queue.id;


--
-- Name: dm_attachments; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dm_attachments (
    id bigint NOT NULL,
    sender_id integer NOT NULL,
    recipient_id integer NOT NULL,
    iv text NOT NULL,
    file_path text NOT NULL,
    original_name text DEFAULT ''::text NOT NULL,
    mime_hint text DEFAULT ''::text NOT NULL,
    size_bytes integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now(),
    expires_at timestamp with time zone DEFAULT (now() + '90 days'::interval)
);


--
-- Name: dm_attachments_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.dm_attachments_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: dm_attachments_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.dm_attachments_id_seq OWNED BY public.dm_attachments.id;


--
-- Name: favorites; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.favorites (
    user_id integer NOT NULL,
    item_id integer NOT NULL
);


--
-- Name: global_chat; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.global_chat (
    id bigint NOT NULL,
    user_id integer NOT NULL,
    message text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: global_chat_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.global_chat_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: global_chat_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.global_chat_id_seq OWNED BY public.global_chat.id;


--
-- Name: global_chat_settings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.global_chat_settings (
    key text NOT NULL,
    value text
);


--
-- Name: halls; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.halls (
    id integer NOT NULL,
    name text NOT NULL,
    slug text NOT NULL,
    created_at timestamp with time zone DEFAULT now(),
    custom_image text,
    description text DEFAULT ''::text NOT NULL,
    rating text DEFAULT 'sfw'::text NOT NULL
);


--
-- Name: halls_assign; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.halls_assign (
    hall_id integer NOT NULL,
    item_id integer NOT NULL,
    user_id integer,
    created_at timestamp with time zone DEFAULT now()
);


--
-- Name: halls_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.halls_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: halls_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.halls_id_seq OWNED BY public.halls.id;


--
-- Name: invite_tokens; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.invite_tokens (
    id integer NOT NULL,
    token character varying(32) NOT NULL,
    created_at bigint NOT NULL,
    created_by integer,
    used_by integer,
    is_used boolean DEFAULT false,
    created_by_discord character varying(255) DEFAULT NULL::character varying,
    created_by_matrix character varying(255) DEFAULT NULL::character varying,
    used_at timestamp with time zone
);


--
-- Name: invite_tokens_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.invite_tokens_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: invite_tokens_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.invite_tokens_id_seq OWNED BY public.invite_tokens.id;


--
-- Name: items_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.items_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: items; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.items (
    id integer DEFAULT nextval('public.items_id_seq'::regclass) NOT NULL,
    src character varying(255) NOT NULL,
    dest character varying(60) NOT NULL,
    mime character varying(100) NOT NULL,
    size integer NOT NULL,
    checksum character varying(255) NOT NULL,
    username character varying(40) NOT NULL,
    userchannel character varying(255) NOT NULL,
    usernetwork character varying(40) NOT NULL,
    stamp integer NOT NULL,
    active boolean NOT NULL,
    thumb character varying(100),
    is_deleted boolean DEFAULT false,
    is_comments_locked boolean DEFAULT false,
    phash text,
    is_purged boolean DEFAULT false,
    has_coverart boolean DEFAULT false,
    is_pinned boolean DEFAULT false,
    is_oc boolean DEFAULT false,
    original_filename text,
    title text,
    width integer,
    height integer,
    visibility smallint DEFAULT 0 NOT NULL,
    slug character varying(16),
    expires_at bigint
);


--
-- Name: COLUMN items.src; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.items.src IS 'src-Link';


--
-- Name: COLUMN items.dest; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.items.dest IS 'filename';


--
-- Name: tags_assign; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tags_assign (
    item_id integer NOT NULL,
    tag_id integer NOT NULL,
    user_id integer DEFAULT 10 NOT NULL
);


--
-- Name: link_token; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.link_token (
    id integer NOT NULL,
    user_id integer NOT NULL,
    token character varying(64) NOT NULL,
    created_at timestamp without time zone DEFAULT CURRENT_TIMESTAMP,
    expires_at timestamp without time zone DEFAULT (CURRENT_TIMESTAMP + '00:10:00'::interval)
);


--
-- Name: link_token_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.link_token_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: link_token_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.link_token_id_seq OWNED BY public.link_token.id;


--
-- Name: login_attempts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.login_attempts (
    id integer NOT NULL,
    ip_hash text NOT NULL,
    username text,
    type text NOT NULL,
    attempted_at timestamp with time zone DEFAULT now(),
    success boolean NOT NULL
);


--
-- Name: login_attempts_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.login_attempts_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: login_attempts_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.login_attempts_id_seq OWNED BY public.login_attempts.id;


--
-- Name: meme_templates; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.meme_templates (
    id integer NOT NULL,
    template_id character varying(255) NOT NULL,
    name character varying(255) NOT NULL,
    url text NOT NULL,
    created_at integer DEFAULT (EXTRACT(epoch FROM now()))::integer,
    category character varying(100) DEFAULT 'General'::character varying,
    sub_category text
);


--
-- Name: meme_templates_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.meme_templates_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: meme_templates_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.meme_templates_id_seq OWNED BY public.meme_templates.id;


--
-- Name: meta_cache; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.meta_cache (
    url text NOT NULL,
    data jsonb,
    created_at timestamp without time zone DEFAULT CURRENT_TIMESTAMP
);


--
-- Name: notifications; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.notifications (
    id integer NOT NULL,
    user_id integer NOT NULL,
    type character varying(32) NOT NULL,
    reference_id integer NOT NULL,
    item_id integer,
    is_read boolean DEFAULT false,
    created_at timestamp with time zone DEFAULT now(),
    data jsonb DEFAULT '{}'::jsonb
);


--
-- Name: notifications_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.notifications_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: notifications_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.notifications_id_seq OWNED BY public.notifications.id;


--
-- Name: private_messages; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.private_messages (
    id bigint NOT NULL,
    sender_id integer NOT NULL,
    recipient_id integer NOT NULL,
    ciphertext text NOT NULL,
    iv text NOT NULL,
    is_read boolean DEFAULT false,
    created_at timestamp with time zone DEFAULT now(),
    edited_at timestamp with time zone
);


--
-- Name: private_messages_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.private_messages_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: private_messages_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.private_messages_id_seq OWNED BY public.private_messages.id;


--
-- Name: reports; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.reports (
    id integer NOT NULL,
    reporter_id integer,
    reporter_ip text,
    item_id integer,
    comment_id integer,
    user_id integer,
    reason text NOT NULL,
    status text DEFAULT 'pending'::text,
    resolved_by integer,
    created_at timestamp with time zone DEFAULT now(),
    categories text[] DEFAULT '{}'::text[]
);


--
-- Name: reports_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.reports_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: reports_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.reports_id_seq OWNED BY public.reports.id;


--
-- Name: site_settings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.site_settings (
    key text NOT NULL,
    value text NOT NULL
);


--
-- Name: sticker_packs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sticker_packs (
    id integer NOT NULL,
    name text NOT NULL,
    tg_name text,
    tg_title text,
    sticker_count integer DEFAULT 0,
    created_at timestamp with time zone DEFAULT now(),
    thumb_url text
);


--
-- Name: sticker_packs_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.sticker_packs_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: sticker_packs_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.sticker_packs_id_seq OWNED BY public.sticker_packs.id;


--
-- Name: tags_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.tags_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: tags; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tags (
    id integer DEFAULT nextval('public.tags_id_seq'::regclass) NOT NULL,
    tag character varying(255) NOT NULL,
    normalized character varying(255)
);


--
-- Name: user_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.user_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: user; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public."user" (
    id integer DEFAULT nextval('public.user_id_seq'::regclass) NOT NULL,
    login character varying(255) NOT NULL,
    "user" character varying(255) NOT NULL,
    password character varying(167) NOT NULL,
    admin boolean NOT NULL,
    created_at timestamp without time zone DEFAULT now() NOT NULL,
    ban_expires timestamp with time zone,
    banned boolean DEFAULT false,
    ban_reason text,
    is_moderator boolean NOT NULL,
    activated boolean DEFAULT true,
    activation_token text,
    email text,
    reset_token text,
    reset_expires timestamp without time zone,
    last_seen integer DEFAULT 0,
    force_password_change boolean DEFAULT false,
    groups text[] DEFAULT '{}'::text[]
);


--
-- Name: user_alias; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_alias (
    userid integer NOT NULL,
    alias character varying(255) NOT NULL,
    type character varying(32) DEFAULT 'discord'::character varying NOT NULL
);

ALTER TABLE ONLY public.user_alias REPLICA IDENTITY FULL;


--
-- Name: user_api_keys; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_api_keys (
    user_id integer NOT NULL,
    api_key text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: user_conversation_states; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_conversation_states (
    user_id integer NOT NULL,
    other_id integer NOT NULL,
    is_hidden boolean DEFAULT false
);


--
-- Name: user_dm_keyvault; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_dm_keyvault (
    user_id integer NOT NULL,
    salt text NOT NULL,
    iv text NOT NULL,
    ciphertext text NOT NULL,
    version smallint DEFAULT 1,
    created_at timestamp with time zone DEFAULT now(),
    updated_at timestamp with time zone DEFAULT now()
);


--
-- Name: user_emoji_favorites; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_emoji_favorites (
    user_id integer NOT NULL,
    emoji_name text NOT NULL,
    emoji_url text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: user_halls; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_halls (
    id integer NOT NULL,
    user_id integer NOT NULL,
    name text NOT NULL,
    slug text NOT NULL,
    description text,
    is_private boolean DEFAULT false NOT NULL,
    custom_image boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: user_halls_assign; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_halls_assign (
    hall_id integer NOT NULL,
    item_id integer NOT NULL,
    user_id integer NOT NULL,
    added_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: user_halls_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.user_halls_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: user_halls_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.user_halls_id_seq OWNED BY public.user_halls.id;


--
-- Name: user_ips; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_ips (
    id integer NOT NULL,
    user_id integer NOT NULL,
    ip text NOT NULL,
    first_seen timestamp with time zone DEFAULT CURRENT_TIMESTAMP,
    last_seen timestamp with time zone DEFAULT CURRENT_TIMESTAMP
);


--
-- Name: user_ips_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.user_ips_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: user_ips_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.user_ips_id_seq OWNED BY public.user_ips.id;


--
-- Name: user_options; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_options (
    user_id integer NOT NULL,
    mode integer NOT NULL,
    theme character varying(50) NOT NULL,
    avatar integer,
    fullscreen smallint DEFAULT '0'::smallint NOT NULL,
    excluded_tags integer[] DEFAULT '{}'::integer[] NOT NULL,
    avatar_file character varying(255),
    show_motd boolean DEFAULT true,
    strict_mode boolean DEFAULT false,
    use_new_layout boolean DEFAULT false,
    username_color character varying,
    font character varying(255) DEFAULT NULL::character varying,
    disable_autoplay boolean DEFAULT false,
    disable_swiping boolean DEFAULT false,
    description text,
    display_name character varying(60),
    show_background boolean,
    ruffle_volume numeric(3,2) DEFAULT NULL::numeric,
    ruffle_background boolean DEFAULT true,
    quote_emojis boolean DEFAULT true NOT NULL,
    embed_youtube_in_comments boolean DEFAULT true NOT NULL,
    hide_koepfe boolean DEFAULT false NOT NULL,
    language text,
    use_alternative_infobox boolean DEFAULT false,
    use_alternative_steuerung boolean,
    receive_system_notifications boolean DEFAULT true,
    receive_user_notifications boolean DEFAULT true,
    do_not_disturb boolean DEFAULT false,
    comment_display_mode integer DEFAULT 1,
    force_comment_display_mode integer DEFAULT 0,
    banner_file character varying(255) DEFAULT NULL::character varying,
    banner_position character varying(50) DEFAULT 'center'::character varying,
    banner_size character varying(50) DEFAULT 'cover'::character varying,
    favorites_private boolean DEFAULT false,
    hide_fav_badge boolean DEFAULT false,
    default_upload_visibility smallint DEFAULT 0,
    banner_repeat character varying(50) DEFAULT 'repeat'::character varying
);


--
-- Name: COLUMN user_options.avatar_file; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.user_options.avatar_file IS 'Custom uploaded avatar filename, stored in public/a/';


--
-- Name: COLUMN user_options.banner_file; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.user_options.banner_file IS 'Custom uploaded banner image filename, stored in public/a/ (same dir as avatars)';


--
-- Name: COLUMN user_options.banner_position; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.user_options.banner_position IS 'CSS background-position for the banner';


--
-- Name: COLUMN user_options.banner_size; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.user_options.banner_size IS 'CSS background-size for the banner (e.g. cover, contain, or a percentage)';


--
-- Name: user_pubkeys; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_pubkeys (
    user_id integer NOT NULL,
    pubkey text NOT NULL,
    fingerprint text,
    created_at timestamp with time zone DEFAULT now(),
    updated_at timestamp with time zone DEFAULT now()
);


--
-- Name: user_sessions_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.user_sessions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: user_sessions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_sessions (
    id integer DEFAULT nextval('public.user_sessions_id_seq'::regclass) NOT NULL,
    user_id integer NOT NULL,
    session character varying(64) NOT NULL,
    browser character varying(255) NOT NULL,
    created_at integer NOT NULL,
    last_used integer NOT NULL,
    last_action character varying(255) NOT NULL,
    kmsi smallint DEFAULT '0'::smallint NOT NULL,
    csrf_token character varying(64),
    ip text,
    passkey_credential_id text
);


--
-- Name: user_video_views; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_video_views (
    user_id integer NOT NULL,
    video_id integer NOT NULL,
    view_count integer DEFAULT 1,
    last_viewed timestamp without time zone DEFAULT now()
);


--
-- Name: user_warnings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_warnings (
    id integer NOT NULL,
    user_id integer,
    admin_id integer,
    reason text NOT NULL,
    created_at timestamp with time zone DEFAULT now(),
    acknowledged boolean DEFAULT false
);


--
-- Name: user_warnings_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.user_warnings_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: user_warnings_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.user_warnings_id_seq OWNED BY public.user_warnings.id;


--
-- Name: webauthn_credentials; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.webauthn_credentials (
    id integer NOT NULL,
    user_id integer NOT NULL,
    cred_id text NOT NULL,
    user_handle uuid NOT NULL,
    passkey jsonb NOT NULL,
    name text,
    aaguid text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_used timestamp with time zone DEFAULT now()
);


--
-- Name: webauthn_credentials_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.webauthn_credentials_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: webauthn_credentials_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.webauthn_credentials_id_seq OWNED BY public.webauthn_credentials.id;


--
-- Name: wordfilter; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.wordfilter (
    id integer NOT NULL,
    word character varying(255) NOT NULL,
    replacement character varying(255) NOT NULL,
    created_at timestamp with time zone DEFAULT now()
);


--
-- Name: wordfilter_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.wordfilter_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: wordfilter_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.wordfilter_id_seq OWNED BY public.wordfilter.id;


--
-- Name: audit_log id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.audit_log ALTER COLUMN id SET DEFAULT nextval('public.audit_log_id_seq'::regclass);


--
-- Name: comment_files id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_files ALTER COLUMN id SET DEFAULT nextval('public.comment_files_id_seq'::regclass);


--
-- Name: comment_poll_options id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_poll_options ALTER COLUMN id SET DEFAULT nextval('public.comment_poll_options_id_seq'::regclass);


--
-- Name: comment_poll_votes id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_poll_votes ALTER COLUMN id SET DEFAULT nextval('public.comment_poll_votes_id_seq'::regclass);


--
-- Name: comment_polls id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_polls ALTER COLUMN id SET DEFAULT nextval('public.comment_polls_id_seq'::regclass);


--
-- Name: comments id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comments ALTER COLUMN id SET DEFAULT nextval('public.comments_id_seq'::regclass);


--
-- Name: custom_emojis id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_emojis ALTER COLUMN id SET DEFAULT nextval('public.custom_emojis_id_seq'::regclass);


--
-- Name: discord_queue id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.discord_queue ALTER COLUMN id SET DEFAULT nextval('public.discord_queue_id_seq'::regclass);


--
-- Name: dm_attachments id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dm_attachments ALTER COLUMN id SET DEFAULT nextval('public.dm_attachments_id_seq'::regclass);


--
-- Name: global_chat id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.global_chat ALTER COLUMN id SET DEFAULT nextval('public.global_chat_id_seq'::regclass);


--
-- Name: halls id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.halls ALTER COLUMN id SET DEFAULT nextval('public.halls_id_seq'::regclass);


--
-- Name: invite_tokens id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invite_tokens ALTER COLUMN id SET DEFAULT nextval('public.invite_tokens_id_seq'::regclass);


--
-- Name: link_token id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.link_token ALTER COLUMN id SET DEFAULT nextval('public.link_token_id_seq'::regclass);


--
-- Name: login_attempts id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.login_attempts ALTER COLUMN id SET DEFAULT nextval('public.login_attempts_id_seq'::regclass);


--
-- Name: meme_templates id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.meme_templates ALTER COLUMN id SET DEFAULT nextval('public.meme_templates_id_seq'::regclass);


--
-- Name: notifications id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notifications ALTER COLUMN id SET DEFAULT nextval('public.notifications_id_seq'::regclass);


--
-- Name: private_messages id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.private_messages ALTER COLUMN id SET DEFAULT nextval('public.private_messages_id_seq'::regclass);


--
-- Name: reports id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports ALTER COLUMN id SET DEFAULT nextval('public.reports_id_seq'::regclass);


--
-- Name: sticker_packs id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sticker_packs ALTER COLUMN id SET DEFAULT nextval('public.sticker_packs_id_seq'::regclass);


--
-- Name: user_halls id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_halls ALTER COLUMN id SET DEFAULT nextval('public.user_halls_id_seq'::regclass);


--
-- Name: user_ips id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_ips ALTER COLUMN id SET DEFAULT nextval('public.user_ips_id_seq'::regclass);


--
-- Name: user_warnings id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_warnings ALTER COLUMN id SET DEFAULT nextval('public.user_warnings_id_seq'::regclass);


--
-- Name: webauthn_credentials id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webauthn_credentials ALTER COLUMN id SET DEFAULT nextval('public.webauthn_credentials_id_seq'::regclass);


--
-- Name: wordfilter id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wordfilter ALTER COLUMN id SET DEFAULT nextval('public.wordfilter_id_seq'::regclass);


--
-- Name: audit_log audit_log_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.audit_log
    ADD CONSTRAINT audit_log_pkey PRIMARY KEY (id);


--
-- Name: comment_files comment_files_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_files
    ADD CONSTRAINT comment_files_pkey PRIMARY KEY (id);


--
-- Name: comment_poll_options comment_poll_options_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_poll_options
    ADD CONSTRAINT comment_poll_options_pkey PRIMARY KEY (id);


--
-- Name: comment_poll_votes comment_poll_votes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_poll_votes
    ADD CONSTRAINT comment_poll_votes_pkey PRIMARY KEY (id);


--
-- Name: comment_poll_votes comment_poll_votes_poll_id_user_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_poll_votes
    ADD CONSTRAINT comment_poll_votes_poll_id_user_id_key UNIQUE (poll_id, user_id);


--
-- Name: comment_polls comment_polls_comment_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_polls
    ADD CONSTRAINT comment_polls_comment_id_key UNIQUE (comment_id);


--
-- Name: comment_polls comment_polls_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_polls
    ADD CONSTRAINT comment_polls_pkey PRIMARY KEY (id);


--
-- Name: comment_subscriptions comment_subscriptions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_subscriptions
    ADD CONSTRAINT comment_subscriptions_pkey PRIMARY KEY (user_id, item_id);


--
-- Name: comments comments_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comments
    ADD CONSTRAINT comments_pkey PRIMARY KEY (id);


--
-- Name: custom_emojis custom_emojis_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_emojis
    ADD CONSTRAINT custom_emojis_name_key UNIQUE (name);


--
-- Name: custom_emojis custom_emojis_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_emojis
    ADD CONSTRAINT custom_emojis_pkey PRIMARY KEY (id);


--
-- Name: discord_queue discord_queue_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.discord_queue
    ADD CONSTRAINT discord_queue_pkey PRIMARY KEY (id);


--
-- Name: dm_attachments dm_attachments_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dm_attachments
    ADD CONSTRAINT dm_attachments_pkey PRIMARY KEY (id);


--
-- Name: global_chat global_chat_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.global_chat
    ADD CONSTRAINT global_chat_pkey PRIMARY KEY (id);


--
-- Name: global_chat_settings global_chat_settings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.global_chat_settings
    ADD CONSTRAINT global_chat_settings_pkey PRIMARY KEY (key);


--
-- Name: halls_assign halls_assign_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.halls_assign
    ADD CONSTRAINT halls_assign_pkey PRIMARY KEY (hall_id, item_id);


--
-- Name: halls halls_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.halls
    ADD CONSTRAINT halls_pkey PRIMARY KEY (id);


--
-- Name: halls halls_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.halls
    ADD CONSTRAINT halls_slug_key UNIQUE (slug);


--
-- Name: favorites idx_16521_primary; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.favorites
    ADD CONSTRAINT idx_16521_primary PRIMARY KEY (user_id, item_id);


--
-- Name: items idx_16526_primary; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.items
    ADD CONSTRAINT idx_16526_primary PRIMARY KEY (id);


--
-- Name: user idx_16554_primary; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public."user"
    ADD CONSTRAINT idx_16554_primary PRIMARY KEY (id);


--
-- Name: user_options idx_16567_user_id; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_options
    ADD CONSTRAINT idx_16567_user_id UNIQUE (user_id);


--
-- Name: user_sessions idx_16572_primary; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_sessions
    ADD CONSTRAINT idx_16572_primary PRIMARY KEY (id);


--
-- Name: invite_tokens invite_tokens_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invite_tokens
    ADD CONSTRAINT invite_tokens_pkey PRIMARY KEY (id);


--
-- Name: invite_tokens invite_tokens_token_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invite_tokens
    ADD CONSTRAINT invite_tokens_token_key UNIQUE (token);


--
-- Name: items items_checksum; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.items
    ADD CONSTRAINT items_checksum UNIQUE (checksum);


--
-- Name: items items_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.items
    ADD CONSTRAINT items_slug_key UNIQUE (slug);


--
-- Name: link_token link_token_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.link_token
    ADD CONSTRAINT link_token_pkey PRIMARY KEY (id);


--
-- Name: link_token link_token_token_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.link_token
    ADD CONSTRAINT link_token_token_key UNIQUE (token);


--
-- Name: login_attempts login_attempts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.login_attempts
    ADD CONSTRAINT login_attempts_pkey PRIMARY KEY (id);


--
-- Name: meme_templates meme_templates_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.meme_templates
    ADD CONSTRAINT meme_templates_pkey PRIMARY KEY (id);


--
-- Name: meme_templates meme_templates_template_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.meme_templates
    ADD CONSTRAINT meme_templates_template_id_key UNIQUE (template_id);


--
-- Name: meta_cache meta_cache_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.meta_cache
    ADD CONSTRAINT meta_cache_pkey PRIMARY KEY (url);


--
-- Name: notifications notifications_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_pkey PRIMARY KEY (id);


--
-- Name: private_messages private_messages_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.private_messages
    ADD CONSTRAINT private_messages_pkey PRIMARY KEY (id);


--
-- Name: reports reports_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_pkey PRIMARY KEY (id);


--
-- Name: site_settings site_settings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.site_settings
    ADD CONSTRAINT site_settings_pkey PRIMARY KEY (key);


--
-- Name: sticker_packs sticker_packs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sticker_packs
    ADD CONSTRAINT sticker_packs_pkey PRIMARY KEY (id);


--
-- Name: sticker_packs sticker_packs_tg_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sticker_packs
    ADD CONSTRAINT sticker_packs_tg_name_key UNIQUE (tg_name);


--
-- Name: tags_assign tags_assign_item_id_tag_id_primary; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tags_assign
    ADD CONSTRAINT tags_assign_item_id_tag_id_primary PRIMARY KEY (item_id, tag_id);


--
-- Name: tags_assign tags_assign_item_id_tag_id_unique; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tags_assign
    ADD CONSTRAINT tags_assign_item_id_tag_id_unique UNIQUE (item_id, tag_id);


--
-- Name: tags tags_id; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tags
    ADD CONSTRAINT tags_id PRIMARY KEY (id);


--
-- Name: user_api_keys user_api_keys_api_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_api_keys
    ADD CONSTRAINT user_api_keys_api_key_key UNIQUE (api_key);


--
-- Name: user_api_keys user_api_keys_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_api_keys
    ADD CONSTRAINT user_api_keys_pkey PRIMARY KEY (user_id);


--
-- Name: user_conversation_states user_conversation_states_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_conversation_states
    ADD CONSTRAINT user_conversation_states_pkey PRIMARY KEY (user_id, other_id);


--
-- Name: user_dm_keyvault user_dm_keyvault_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_dm_keyvault
    ADD CONSTRAINT user_dm_keyvault_pkey PRIMARY KEY (user_id);


--
-- Name: user user_email_unique; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public."user"
    ADD CONSTRAINT user_email_unique UNIQUE (email);


--
-- Name: user_emoji_favorites user_emoji_favorites_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_emoji_favorites
    ADD CONSTRAINT user_emoji_favorites_pkey PRIMARY KEY (user_id, emoji_name);


--
-- Name: user_halls_assign user_halls_assign_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_halls_assign
    ADD CONSTRAINT user_halls_assign_pkey PRIMARY KEY (hall_id, item_id);


--
-- Name: user_halls user_halls_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_halls
    ADD CONSTRAINT user_halls_pkey PRIMARY KEY (id);


--
-- Name: user_halls user_halls_user_id_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_halls
    ADD CONSTRAINT user_halls_user_id_slug_key UNIQUE (user_id, slug);


--
-- Name: user_ips user_ips_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_ips
    ADD CONSTRAINT user_ips_pkey PRIMARY KEY (id);


--
-- Name: user_ips user_ips_user_id_ip_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_ips
    ADD CONSTRAINT user_ips_user_id_ip_key UNIQUE (user_id, ip);


--
-- Name: user user_login_unique; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public."user"
    ADD CONSTRAINT user_login_unique UNIQUE (login);


--
-- Name: user user_name_unique; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public."user"
    ADD CONSTRAINT user_name_unique UNIQUE ("user");


--
-- Name: user_options user_options_user_id; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_options
    ADD CONSTRAINT user_options_user_id PRIMARY KEY (user_id);


--
-- Name: user_pubkeys user_pubkeys_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_pubkeys
    ADD CONSTRAINT user_pubkeys_pkey PRIMARY KEY (user_id);


--
-- Name: user_video_views user_video_views_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_video_views
    ADD CONSTRAINT user_video_views_pkey PRIMARY KEY (user_id, video_id);


--
-- Name: user_warnings user_warnings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_warnings
    ADD CONSTRAINT user_warnings_pkey PRIMARY KEY (id);


--
-- Name: webauthn_credentials webauthn_credentials_cred_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webauthn_credentials
    ADD CONSTRAINT webauthn_credentials_cred_id_key UNIQUE (cred_id);


--
-- Name: webauthn_credentials webauthn_credentials_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webauthn_credentials
    ADD CONSTRAINT webauthn_credentials_pkey PRIMARY KEY (id);


--
-- Name: wordfilter wordfilter_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wordfilter
    ADD CONSTRAINT wordfilter_pkey PRIMARY KEY (id);


--
-- Name: wordfilter wordfilter_word_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wordfilter
    ADD CONSTRAINT wordfilter_word_key UNIQUE (word);


--
-- Name: custom_emojis_pack_id_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX custom_emojis_pack_id_idx ON public.custom_emojis USING btree (pack_id);


--
-- Name: idx_audit_log_action; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_audit_log_action ON public.audit_log USING btree (action);


--
-- Name: idx_audit_log_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_audit_log_created_at ON public.audit_log USING btree (created_at);


--
-- Name: idx_audit_log_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_audit_log_user_id ON public.audit_log USING btree (user_id);


--
-- Name: idx_comment_files_checksum; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_comment_files_checksum ON public.comment_files USING btree (checksum);


--
-- Name: idx_comment_files_comment_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_comment_files_comment_id ON public.comment_files USING btree (comment_id);


--
-- Name: idx_comment_poll_options_poll; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_comment_poll_options_poll ON public.comment_poll_options USING btree (poll_id);


--
-- Name: idx_comment_poll_votes_poll; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_comment_poll_votes_poll ON public.comment_poll_votes USING btree (poll_id);


--
-- Name: idx_comment_poll_votes_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_comment_poll_votes_user ON public.comment_poll_votes USING btree (user_id);


--
-- Name: idx_comments_is_pinned; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_comments_is_pinned ON public.comments USING btree (is_pinned) WHERE (is_pinned = true);


--
-- Name: idx_comments_item_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_comments_item_id ON public.comments USING btree (item_id);


--
-- Name: idx_comments_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_comments_user_id ON public.comments USING btree (user_id);


--
-- Name: idx_discord_queue_sent; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_discord_queue_sent ON public.discord_queue USING btree (sent) WHERE (sent = false);


--
-- Name: idx_dm_att_expires; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_dm_att_expires ON public.dm_attachments USING btree (expires_at);


--
-- Name: idx_dm_att_recipient; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_dm_att_recipient ON public.dm_attachments USING btree (recipient_id);


--
-- Name: idx_dm_att_sender; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_dm_att_sender ON public.dm_attachments USING btree (sender_id);


--
-- Name: idx_global_chat_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_global_chat_created_at ON public.global_chat USING btree (created_at DESC);


--
-- Name: idx_invite_tokens_created_by; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_invite_tokens_created_by ON public.invite_tokens USING btree (created_by);


--
-- Name: idx_invite_tokens_used_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_invite_tokens_used_at ON public.invite_tokens USING btree (used_at) WHERE (is_used = true);


--
-- Name: idx_items_active_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_items_active_id ON public.items USING btree (id) WHERE (active = true);


--
-- Name: idx_items_expires_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_items_expires_at ON public.items USING btree (expires_at) WHERE ((expires_at IS NOT NULL) AND (is_purged = false));


--
-- Name: idx_items_is_deleted; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_items_is_deleted ON public.items USING btree (is_deleted) WHERE (is_deleted = false);


--
-- Name: idx_items_is_purged; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_items_is_purged ON public.items USING btree (is_purged) WHERE (is_purged = true);


--
-- Name: idx_items_pinned_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_items_pinned_id ON public.items USING btree (is_pinned DESC, id DESC);


--
-- Name: idx_items_username; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_items_username ON public.items USING btree (username);


--
-- Name: idx_items_username_lower; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_items_username_lower ON public.items USING btree (lower((username)::text));



--
-- Name: idx_login_attempts_ip_time; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_login_attempts_ip_time ON public.login_attempts USING btree (ip_hash, attempted_at DESC);


--
-- Name: idx_login_attempts_username_time; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_login_attempts_username_time ON public.login_attempts USING btree (username, attempted_at DESC);


--
-- Name: idx_notifications_unread; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_notifications_unread ON public.notifications USING btree (user_id) WHERE (is_read = false);


--
-- Name: idx_notifications_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_notifications_user_id ON public.notifications USING btree (user_id);


--
-- Name: idx_pm_conversation; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_pm_conversation ON public.private_messages USING btree (LEAST(sender_id, recipient_id), GREATEST(sender_id, recipient_id), created_at DESC);


--
-- Name: idx_pm_recipient; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_pm_recipient ON public.private_messages USING btree (recipient_id, created_at DESC);


--
-- Name: idx_pm_sender; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_pm_sender ON public.private_messages USING btree (sender_id, created_at DESC);


--
-- Name: idx_reports_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_reports_status ON public.reports USING btree (status);


--
-- Name: idx_ucs_user_hidden; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_ucs_user_hidden ON public.user_conversation_states USING btree (user_id, is_hidden);


--
-- Name: idx_user_alias_alias; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_user_alias_alias ON public.user_alias USING btree (alias);


--
-- Name: idx_user_alias_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_user_alias_type ON public.user_alias USING btree (type);


--
-- Name: idx_user_alias_userid; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_user_alias_userid ON public.user_alias USING btree (userid);


--
-- Name: idx_user_emoji_favorites_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_user_emoji_favorites_user_id ON public.user_emoji_favorites USING btree (user_id);


--
-- Name: idx_user_halls_assign_hall; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_user_halls_assign_hall ON public.user_halls_assign USING btree (hall_id);


--
-- Name: idx_user_halls_assign_item; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_user_halls_assign_item ON public.user_halls_assign USING btree (item_id);


--
-- Name: idx_user_halls_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_user_halls_user_id ON public.user_halls USING btree (user_id);


--
-- Name: idx_user_ips_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_user_ips_user_id ON public.user_ips USING btree (user_id);


--
-- Name: idx_user_last_seen; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_user_last_seen ON public."user" USING btree (last_seen);


--
-- Name: idx_user_warnings_user_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_user_warnings_user_status ON public.user_warnings USING btree (user_id, acknowledged);


--
-- Name: idx_webauthn_credentials_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_webauthn_credentials_user_id ON public.webauthn_credentials USING btree (user_id);


--
-- Name: items_phash; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX items_phash ON public.items USING btree (phash);


--
-- Name: tags_assign_tag_id_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tags_assign_tag_id_idx ON public.tags_assign USING btree (tag_id);


--
-- Name: global_chat global_chat_notify; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER global_chat_notify AFTER INSERT ON public.global_chat FOR EACH ROW EXECUTE FUNCTION public.notify_global_chat();


--
-- Name: tags_assign tags_assign_ad; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER tags_assign_ad AFTER DELETE ON public.tags_assign FOR EACH ROW EXECUTE FUNCTION public.delete_unused_tags();


--
-- Name: tags tags_bi; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER tags_bi BEFORE INSERT ON public.tags FOR EACH ROW EXECUTE FUNCTION public.fill_normalized();


--
-- Name: tags tags_bu; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER tags_bu BEFORE UPDATE ON public.tags FOR EACH ROW EXECUTE FUNCTION public.fill_normalized();



--
-- Name: comments trigger_notify_activity; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER trigger_notify_activity AFTER INSERT ON public.comments FOR EACH ROW EXECUTE FUNCTION public.notify_activity();


--
-- Name: notifications trigger_notify_notification; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER trigger_notify_notification AFTER INSERT OR UPDATE ON public.notifications FOR EACH ROW EXECUTE FUNCTION public.notify_notification();


--
-- Name: private_messages trigger_notify_private_message; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER trigger_notify_private_message AFTER INSERT ON public.private_messages FOR EACH ROW EXECUTE FUNCTION public.notify_private_message();


--
-- Name: comment_files comment_files_comment_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_files
    ADD CONSTRAINT comment_files_comment_id_fkey FOREIGN KEY (comment_id) REFERENCES public.comments(id) ON DELETE CASCADE;


--
-- Name: comment_files comment_files_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_files
    ADD CONSTRAINT comment_files_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: comment_poll_options comment_poll_options_poll_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_poll_options
    ADD CONSTRAINT comment_poll_options_poll_id_fkey FOREIGN KEY (poll_id) REFERENCES public.comment_polls(id) ON DELETE CASCADE;


--
-- Name: comment_poll_votes comment_poll_votes_option_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_poll_votes
    ADD CONSTRAINT comment_poll_votes_option_id_fkey FOREIGN KEY (option_id) REFERENCES public.comment_poll_options(id) ON DELETE CASCADE;


--
-- Name: comment_poll_votes comment_poll_votes_poll_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_poll_votes
    ADD CONSTRAINT comment_poll_votes_poll_id_fkey FOREIGN KEY (poll_id) REFERENCES public.comment_polls(id) ON DELETE CASCADE;


--
-- Name: comment_poll_votes comment_poll_votes_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_poll_votes
    ADD CONSTRAINT comment_poll_votes_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: comment_polls comment_polls_comment_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_polls
    ADD CONSTRAINT comment_polls_comment_id_fkey FOREIGN KEY (comment_id) REFERENCES public.comments(id) ON DELETE CASCADE;


--
-- Name: comment_subscriptions comment_subscriptions_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_subscriptions
    ADD CONSTRAINT comment_subscriptions_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE;


--
-- Name: comment_subscriptions comment_subscriptions_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comment_subscriptions
    ADD CONSTRAINT comment_subscriptions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: comments comments_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comments
    ADD CONSTRAINT comments_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE;


--
-- Name: comments comments_parent_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comments
    ADD CONSTRAINT comments_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES public.comments(id) ON DELETE SET NULL;


--
-- Name: comments comments_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.comments
    ADD CONSTRAINT comments_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: custom_emojis custom_emojis_pack_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_emojis
    ADD CONSTRAINT custom_emojis_pack_id_fkey FOREIGN KEY (pack_id) REFERENCES public.sticker_packs(id) ON DELETE SET NULL;


--
-- Name: discord_queue discord_queue_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.discord_queue
    ADD CONSTRAINT discord_queue_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE;


--
-- Name: discord_queue discord_queue_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.discord_queue
    ADD CONSTRAINT discord_queue_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: dm_attachments dm_attachments_recipient_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dm_attachments
    ADD CONSTRAINT dm_attachments_recipient_id_fkey FOREIGN KEY (recipient_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: dm_attachments dm_attachments_sender_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dm_attachments
    ADD CONSTRAINT dm_attachments_sender_id_fkey FOREIGN KEY (sender_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: favorites favorites_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.favorites
    ADD CONSTRAINT favorites_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE;


--
-- Name: favorites favorites_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.favorites
    ADD CONSTRAINT favorites_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: global_chat global_chat_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.global_chat
    ADD CONSTRAINT global_chat_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: halls_assign halls_assign_hall_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.halls_assign
    ADD CONSTRAINT halls_assign_hall_id_fkey FOREIGN KEY (hall_id) REFERENCES public.halls(id) ON DELETE CASCADE;


--
-- Name: halls_assign halls_assign_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.halls_assign
    ADD CONSTRAINT halls_assign_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE;


--
-- Name: halls_assign halls_assign_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.halls_assign
    ADD CONSTRAINT halls_assign_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE SET NULL;


--
-- Name: invite_tokens invite_tokens_created_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invite_tokens
    ADD CONSTRAINT invite_tokens_created_by_fkey FOREIGN KEY (created_by) REFERENCES public."user"(id);


--
-- Name: invite_tokens invite_tokens_used_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invite_tokens
    ADD CONSTRAINT invite_tokens_used_by_fkey FOREIGN KEY (used_by) REFERENCES public."user"(id);


--
-- Name: link_token link_token_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.link_token
    ADD CONSTRAINT link_token_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: notifications notifications_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE;


--
-- Name: notifications notifications_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: private_messages private_messages_recipient_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.private_messages
    ADD CONSTRAINT private_messages_recipient_id_fkey FOREIGN KEY (recipient_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: private_messages private_messages_sender_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.private_messages
    ADD CONSTRAINT private_messages_sender_id_fkey FOREIGN KEY (sender_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: reports reports_comment_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_comment_id_fkey FOREIGN KEY (comment_id) REFERENCES public.comments(id) ON DELETE CASCADE;


--
-- Name: reports reports_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE;


--
-- Name: reports reports_reporter_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_reporter_id_fkey FOREIGN KEY (reporter_id) REFERENCES public."user"(id) ON DELETE SET NULL;


--
-- Name: reports reports_resolved_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_resolved_by_fkey FOREIGN KEY (resolved_by) REFERENCES public."user"(id) ON DELETE SET NULL;


--
-- Name: reports reports_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: tags_assign tags_assign_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tags_assign
    ADD CONSTRAINT tags_assign_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE;


--
-- Name: tags_assign tags_assign_tag_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tags_assign
    ADD CONSTRAINT tags_assign_tag_id_fkey FOREIGN KEY (tag_id) REFERENCES public.tags(id) ON DELETE CASCADE;


--
-- Name: tags_assign tags_assign_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tags_assign
    ADD CONSTRAINT tags_assign_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE SET DEFAULT;


--
-- Name: user_api_keys user_api_keys_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_api_keys
    ADD CONSTRAINT user_api_keys_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: user_conversation_states user_conversation_states_other_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_conversation_states
    ADD CONSTRAINT user_conversation_states_other_id_fkey FOREIGN KEY (other_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: user_conversation_states user_conversation_states_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_conversation_states
    ADD CONSTRAINT user_conversation_states_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: user_dm_keyvault user_dm_keyvault_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_dm_keyvault
    ADD CONSTRAINT user_dm_keyvault_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: user_emoji_favorites user_emoji_favorites_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_emoji_favorites
    ADD CONSTRAINT user_emoji_favorites_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: user_halls_assign user_halls_assign_hall_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_halls_assign
    ADD CONSTRAINT user_halls_assign_hall_id_fkey FOREIGN KEY (hall_id) REFERENCES public.user_halls(id) ON DELETE CASCADE;


--
-- Name: user_halls_assign user_halls_assign_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_halls_assign
    ADD CONSTRAINT user_halls_assign_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE;


--
-- Name: user_halls_assign user_halls_assign_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_halls_assign
    ADD CONSTRAINT user_halls_assign_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: user_halls user_halls_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_halls
    ADD CONSTRAINT user_halls_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: user_ips user_ips_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_ips
    ADD CONSTRAINT user_ips_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: user_options user_options_avatar_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_options
    ADD CONSTRAINT user_options_avatar_fkey FOREIGN KEY (avatar) REFERENCES public.items(id) ON DELETE SET DEFAULT;


--
-- Name: user_options user_options_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_options
    ADD CONSTRAINT user_options_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: user_pubkeys user_pubkeys_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_pubkeys
    ADD CONSTRAINT user_pubkeys_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: user_sessions user_sessions_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_sessions
    ADD CONSTRAINT user_sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: user_video_views user_video_views_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_video_views
    ADD CONSTRAINT user_video_views_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: user_video_views user_video_views_video_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_video_views
    ADD CONSTRAINT user_video_views_video_id_fkey FOREIGN KEY (video_id) REFERENCES public.items(id) ON DELETE CASCADE;


--
-- Name: webauthn_credentials webauthn_credentials_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webauthn_credentials
    ADD CONSTRAINT webauthn_credentials_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- Name: user_warnings user_warnings_admin_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_warnings
    ADD CONSTRAINT user_warnings_admin_id_fkey FOREIGN KEY (admin_id) REFERENCES public."user"(id) ON DELETE SET NULL;


--
-- Name: user_warnings user_warnings_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_warnings
    ADD CONSTRAINT user_warnings_user_id_fkey FOREIGN KEY (user_id) REFERENCES public."user"(id) ON DELETE CASCADE;


--
-- ============================================================
-- Migrations not present in the original pg_dump
-- ============================================================

-- Migration: anon_identities
CREATE TABLE IF NOT EXISTS public.anon_identities (
    id SERIAL PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES public."user"(id) ON DELETE CASCADE,
    pubkey TEXT UNIQUE,
    fingerprint TEXT UNIQUE,
    credential_id TEXT UNIQUE,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    last_seen TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    created_ip VARCHAR(128),
    last_ip VARCHAR(128),
    hw_fingerprint VARCHAR(128)
);

CREATE INDEX IF NOT EXISTS idx_anon_identities_user_id ON public.anon_identities(user_id);
CREATE INDEX IF NOT EXISTS idx_anon_identities_hw ON public.anon_identities(hw_fingerprint);

-- Migration: anon_activity_log and ban tables
CREATE TABLE IF NOT EXISTS anon_activity_log (
    id SERIAL PRIMARY KEY,
    user_id INT NOT NULL,
    fingerprint VARCHAR(128),
    hw_fingerprint VARCHAR(128),
    action VARCHAR(64) NOT NULL,
    target_id INT,
    ip VARCHAR(128),
    user_agent TEXT,
    details JSONB,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_anon_act_user_id ON anon_activity_log (user_id);
CREATE INDEX IF NOT EXISTS idx_anon_act_action ON anon_activity_log (action);
CREATE INDEX IF NOT EXISTS idx_anon_act_ip ON anon_activity_log (ip);
CREATE INDEX IF NOT EXISTS idx_anon_act_created ON anon_activity_log (created_at);
CREATE INDEX IF NOT EXISTS idx_anon_activity_hw ON anon_activity_log(hw_fingerprint);

CREATE TABLE IF NOT EXISTS banned_ips (
    id SERIAL PRIMARY KEY,
    ip VARCHAR(128) NOT NULL UNIQUE,
    ip_hash VARCHAR(128),
    reason TEXT DEFAULT 'Banned by administrator',
    banned_by INT,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    expires_at TIMESTAMP WITH TIME ZONE
);

CREATE INDEX IF NOT EXISTS idx_banned_ips_hash ON banned_ips (ip_hash);

CREATE TABLE IF NOT EXISTS banned_fingerprints (
    id SERIAL PRIMARY KEY,
    fingerprint VARCHAR(128) NOT NULL UNIQUE,
    pubkey TEXT,
    reason TEXT DEFAULT 'Banned by administrator',
    banned_by INT,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    expires_at TIMESTAMP WITH TIME ZONE
);


CREATE TABLE IF NOT EXISTS banned_hardware_fingerprints (
    id SERIAL PRIMARY KEY,
    hw_fingerprint VARCHAR(128) UNIQUE NOT NULL,
    banned_by INT REFERENCES "user"(id) ON DELETE SET NULL,
    reason TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_banned_hw_expires ON banned_hardware_fingerprints(expires_at);

-- Add IP columns to comments/items if missing
ALTER TABLE comments ADD COLUMN IF NOT EXISTS ip VARCHAR(128);
ALTER TABLE items ADD COLUMN IF NOT EXISTS uploader_ip VARCHAR(128);

-- Migration: album support
ALTER TABLE public.items ADD COLUMN IF NOT EXISTS is_album boolean DEFAULT false;
ALTER TABLE public.items ADD COLUMN IF NOT EXISTS album_count integer DEFAULT 0;

CREATE TABLE IF NOT EXISTS public.album_items (
    id SERIAL PRIMARY KEY,
    item_id INTEGER NOT NULL REFERENCES public.items(id) ON DELETE CASCADE,
    dest CHARACTER VARYING(60) NOT NULL,
    mime CHARACTER VARYING(100) NOT NULL,
    size INTEGER NOT NULL,
    checksum CHARACTER VARYING(255) NOT NULL,
    phash TEXT,
    width INTEGER,
    height INTEGER,
    order_index INTEGER NOT NULL DEFAULT 0,
    slug CHARACTER VARYING(60) DEFAULT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_album_items_item_id ON public.album_items(item_id, order_index ASC);
CREATE INDEX IF NOT EXISTS idx_album_items_slug ON public.album_items(slug);

CREATE TABLE IF NOT EXISTS public.album_items_tags_assign (
    album_item_id INTEGER NOT NULL REFERENCES public.album_items(id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES public.tags(id) ON DELETE CASCADE,
    user_id INTEGER REFERENCES public."user"(id) ON DELETE SET DEFAULT,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    PRIMARY KEY (album_item_id, tag_id)
);

CREATE INDEX IF NOT EXISTS idx_album_items_tags_assign_tag_id ON public.album_items_tags_assign(tag_id);
CREATE INDEX IF NOT EXISTS idx_album_items_tags_assign_album_item_id ON public.album_items_tags_assign(album_item_id);

-- Migration: user recommendation affinity
CREATE TABLE IF NOT EXISTS public.user_tag_affinity (
    user_id integer NOT NULL REFERENCES "user"(id) ON DELETE CASCADE,
    tag_id integer NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    score real DEFAULT 0.0,
    interaction_count integer DEFAULT 1,
    last_interacted timestamp with time zone DEFAULT now(),
    PRIMARY KEY (user_id, tag_id)
);

CREATE INDEX IF NOT EXISTS idx_user_tag_affinity_user_score
ON public.user_tag_affinity(user_id, score DESC);

CREATE TABLE IF NOT EXISTS public.user_creator_affinity (
    user_id integer NOT NULL REFERENCES "user"(id) ON DELETE CASCADE,
    creator_username text NOT NULL,
    score real DEFAULT 0.0,
    interaction_count integer DEFAULT 1,
    last_interacted timestamp with time zone DEFAULT now(),
    PRIMARY KEY (user_id, creator_username)
);

CREATE INDEX IF NOT EXISTS idx_user_creator_affinity_user_score
ON public.user_creator_affinity(user_id, score DESC);

-- ============================================================
-- End of consolidated migrations
-- ============================================================

--
-- Name: alltables; Type: PUBLICATION; Schema: -; Owner: -
--

CREATE PUBLICATION alltables FOR ALL TABLES WITH (publish = 'insert, update, delete, truncate');


--
-- Name: invite_requests; Type: TABLE; Schema: public
--

CREATE TABLE IF NOT EXISTS public.invite_requests (
    id SERIAL PRIMARY KEY,
    user_id INTEGER REFERENCES public."user"(id) ON DELETE SET NULL,
    fingerprint VARCHAR(128) NOT NULL,
    ip_hash VARCHAR(128),
    reason TEXT DEFAULT '',
    status VARCHAR(16) DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'denied', 'revoked')),
    token_id INTEGER REFERENCES public.invite_tokens(id) ON DELETE SET NULL,
    reviewed_by INTEGER REFERENCES public."user"(id) ON DELETE SET NULL,
    reviewed_at TIMESTAMP WITH TIME ZONE,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_invite_requests_fingerprint ON public.invite_requests(fingerprint);
CREATE INDEX IF NOT EXISTS idx_invite_requests_status ON public.invite_requests(status);
CREATE INDEX IF NOT EXISTS idx_invite_requests_pending ON public.invite_requests(created_at) WHERE (status = 'pending');


--
-- PostgreSQL database dump complete
--

