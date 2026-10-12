package api

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"quirenote/internal/setup"
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

func TestFirstRunSetup(t *testing.T) {
	home := t.TempDir()
	s := New(nil, watch.NewBroker(), nil)
	var opened string
	s.EnableSetup(home, func(res setup.Result) error {
		v, err := vault.Open(res.Dir)
		if err != nil {
			return err
		}
		t.Cleanup(func() { _ = v.Close() })
		opened = res.Dir
		s.SetVault(v)
		return nil
	})
	h := s.Handler()
	do := func(method, path, body string) (int, string) {
		req := httptest.NewRequest(method, path, strings.NewReader(body))
		req.Header.Set("Content-Type", "application/json")
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, req)
		return rec.Code, rec.Body.String()
	}

	// Without a vault, the vault endpoints point to the setup.
	if code, body := do(http.MethodGet, "/api/tree", ""); code != http.StatusConflict || !strings.Contains(body, `"setup":true`) {
		t.Fatalf("tree before setup: %d %s", code, body)
	}
	if code, body := do(http.MethodGet, "/api/setup", ""); code != http.StatusOK || !strings.Contains(body, `"configured":false`) {
		t.Fatalf("setup status: %d %s", code, body)
	}
	if code, _ := do(http.MethodPost, "/api/setup", `{"mode":"existing","path":"/"}`); code != http.StatusBadRequest {
		t.Fatalf("setup with / : %d", code)
	}
	if code, body := do(http.MethodPost, "/api/setup", `{"mode":"new","path":"~/Notes"}`); code != http.StatusOK || opened == "" {
		t.Fatalf("setup: %d %s", code, body)
	}
	if code, body := do(http.MethodGet, "/api/tree", ""); code != http.StatusOK || !strings.Contains(body, "Welcome.md") {
		t.Fatalf("tree after setup: %d %s", code, body)
	}
	// Once configured, the browser cannot point the server elsewhere.
	if code, _ := do(http.MethodPost, "/api/setup", `{"mode":"existing","path":"~"}`); code != http.StatusForbidden {
		t.Fatalf("second setup: %d", code)
	}
}
