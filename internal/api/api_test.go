package api

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"quirenote/internal/vault"
	"quirenote/internal/watch"
)

func TestMutationsRequireJSON(t *testing.T) {
	v, err := vault.Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = v.Close() })
	h := New(v, watch.NewBroker(), nil).Handler()

	do := func(method, path, ct, body string) int {
		req := httptest.NewRequest(method, path, strings.NewReader(body))
		if ct != "" {
			req.Header.Set("Content-Type", ct)
		}
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, req)
		return rec.Code
	}

	// What a form/no-cors fetch from another page would do: rejected.
	for _, ct := range []string{"", "text/plain", "application/x-www-form-urlencoded"} {
		if code := do(http.MethodPost, "/api/notes/csrf.md", ct, `{"content":"x"}`); code != http.StatusUnsupportedMediaType {
			t.Errorf("POST with Content-Type %q: status %d, want 415", ct, code)
		}
		if code := do(http.MethodPost, "/api/dirs/csrf", ct, `{}`); code != http.StatusUnsupportedMediaType {
			t.Errorf("POST /api/dirs with Content-Type %q: status %d, want 415", ct, code)
		}
	}
	if code := do(http.MethodPost, "/api/notes/ok.md", "application/json; charset=utf-8", `{"content":"x"}`); code != http.StatusCreated {
		t.Errorf("POST JSON: status %d, want 201", code)
	}
	if code := do(http.MethodPost, "/api/notes/ok.md", "application/json", `{"content":"y"}`); code != http.StatusConflict {
		t.Errorf("repeated POST JSON: status %d, want 409", code)
	}
	if code := do(http.MethodGet, "/api/tree", "", ""); code != http.StatusOK {
		t.Errorf("GET without Content-Type: status %d, want 200", code)
	}
}
