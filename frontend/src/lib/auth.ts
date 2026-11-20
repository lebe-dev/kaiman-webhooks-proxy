const TOKEN_COOKIE = "kwp_token";

export function getToken(): string | null {
  const match = new RegExp(`(?:^|; )${TOKEN_COOKIE}=([^;]*)`).exec(
    document.cookie
  );
  return match ? decodeURIComponent(match[1]) : null;
}

export function setToken(token: string): void {
  document.cookie = `${TOKEN_COOKIE}=${encodeURIComponent(token)}; path=/; SameSite=Strict`;
}

export function clearToken(): void {
  document.cookie = `${TOKEN_COOKIE}=; path=/; max-age=0`;
}

export type AuthStatus =
  | { authType: "token" }
  | { authType: "oauth"; provider: string; authenticated: boolean };

export async function fetchAuthStatus(): Promise<AuthStatus> {
  const res = await fetch("/api/auth/status");
  if (!res.ok) throw new Error("Failed to check login status");
  return res.json();
}

export async function logoutOAuth(): Promise<void> {
  const res = await fetch("/api/auth/logout", { method: "POST" });
  if (!res.ok) throw new Error("Failed to log out");
}
