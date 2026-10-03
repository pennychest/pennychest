import { QueryClient, useMutation, useQueryClient } from "@tanstack/react-query";
import { authApi } from "../api/client";

export const AUTH_QUERY_KEY = ["auth-status"];
export const MIN_PASSWORD_LENGTH = 8;

// Drops everything cached under the previous session so no data outlives it.
export function setSignedIn(queryClient: QueryClient, authenticated: boolean) {
  queryClient.removeQueries({ predicate: (q) => q.queryKey[0] !== AUTH_QUERY_KEY[0] });
  queryClient.setQueryData(AUTH_QUERY_KEY, { setup_required: false, authenticated });
}

export function useSignOut() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: authApi.logout,
    onSettled: () => setSignedIn(queryClient, false),
  });
}
