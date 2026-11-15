-- Migration: account_applications
-- Guests applying for an account with a message instead of an invite token.
-- The "user" row is only created when an admin approves the application.

CREATE TABLE IF NOT EXISTS public.account_applications (
    id SERIAL PRIMARY KEY,
    login VARCHAR(255) NOT NULL,
    username VARCHAR(255) NOT NULL,
    password TEXT NOT NULL DEFAULT '',
    email TEXT,
    message TEXT NOT NULL DEFAULT '',
    ip_hash VARCHAR(128),
    status VARCHAR(16) NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'denied')),
    user_id INTEGER REFERENCES public."user"(id) ON DELETE SET NULL,
    reviewed_by INTEGER REFERENCES public."user"(id) ON DELETE SET NULL,
    reviewed_at TIMESTAMP WITH TIME ZONE,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_account_applications_pending_login ON public.account_applications(login) WHERE (status = 'pending');
CREATE INDEX IF NOT EXISTS idx_account_applications_login ON public.account_applications(login);
CREATE INDEX IF NOT EXISTS idx_account_applications_status ON public.account_applications(status, created_at);
