<script lang="ts">
    import { onMount, type Snippet } from "svelte";
    import { clearToken, fetchAuthStatus, getToken, logoutOAuth, setToken, type AuthStatus } from "$lib/auth";
    import { Input } from "$lib/components/ui/input";
    import { Button } from "$lib/components/ui/button";
    import { toast } from "svelte-sonner";

    let { children }: { children: Snippet<[() => Promise<void>, string | undefined]> } = $props();

    let authStatus = $state<AuthStatus | null>(null);
    let statusError = $state(false);
    let authenticated = $state(false);
    let tokenInput = $state("");
    let connecting = $state(false);
    let loginError = $state("");

    onMount(() => {
        const url = new URL(window.location.href);
        const error = url.searchParams.get("oauth_error");
        if (error) {
            loginError = error === "denied"
                ? "This account was not granted access."
                : "OAuth sign-in failed. Please try again.";
            url.searchParams.delete("oauth_error");
            window.history.replaceState(window.history.state, "", url);
        }
        void loadAuthStatus();
    });

    async function loadAuthStatus() {
        statusError = false;
        try {
            authStatus = await fetchAuthStatus();
            if (authStatus.authType === "oauth") {
                clearToken();
                authenticated = authStatus.authenticated;
            } else {
                authenticated = !!getToken();
            }
        } catch {
            statusError = true;
        }
    }

    async function connect(event: SubmitEvent) {
        event.preventDefault();
        if (!tokenInput.trim() || connecting) return;
        connecting = true;
        try {
            const res = await fetch("/api/config", {
                headers: { Authorization: `Bearer ${tokenInput}` },
            });
            if (res.ok) {
                setToken(tokenInput);
                authenticated = true;
            } else {
                toast.error("Invalid token");
            }
        } catch {
            toast.error("Connection failed");
        } finally {
            connecting = false;
        }
    }

    async function logout() {
        try {
            if (authStatus?.authType === "oauth") await logoutOAuth();
            clearToken();
            window.location.reload();
        } catch {
            toast.error("Could not log out. Please try again.");
        }
    }
</script>

{#if authenticated}
    {@render children(logout, authStatus?.authType === "oauth" ? authStatus.username : undefined)}
{:else}
    <div class="flex min-h-screen items-center justify-center p-4">
        <div class="w-full max-w-sm space-y-4 rounded-lg border bg-card text-card-foreground shadow-card p-6">
            <h1 class="text-2xl font-bold text-center">
                <span class="text-gray-500">Kaiman</span>
                Webhooks Proxy
            </h1>
            {#if authStatus?.authType === "oauth" && loginError}
                <p role="alert" class="text-sm text-destructive text-center">{loginError}</p>
            {/if}
            {#if statusError}
                <p role="alert" class="text-sm text-destructive text-center">Could not check login status.</p>
                <Button class="w-full" onclick={loadAuthStatus}>Retry</Button>
            {:else if authStatus}
                {#if authStatus.authType === "oauth"}
                    <Button class="w-full" href="/api/auth/oauth/start">
                        Sign in with {authStatus.provider}
                    </Button>
                {:else}
                    <form class="space-y-4" onsubmit={connect}>
                        <Input
                            type="password"
                            aria-label="Access token"
                            placeholder="Access token"
                            bind:value={tokenInput}
                        />
                        <Button class="w-full" type="submit" disabled={connecting}>
                            {connecting ? "Connecting..." : "Sign in with token"}
                        </Button>
                    </form>
                {/if}
            {:else}
                <p role="status" class="text-sm text-muted-foreground text-center">Checking login status...</p>
            {/if}
        </div>
    </div>
{/if}
