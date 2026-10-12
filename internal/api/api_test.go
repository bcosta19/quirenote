package api

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"notas/internal/vault"
	"notas/internal/watch"
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

	// Como um formulário/fetch no-cors de outra página faria: recusado.
	for _, ct := range []string{"", "text/plain", "application/x-www-form-urlencoded"} {
		if code := do(http.MethodPost, "/api/notes/csrf.md", ct, `{"content":"x"}`); code != http.StatusUnsupportedMediaType {
			t.Errorf("POST com Content-Type %q: código %d, esperava 415", ct, code)
		}
		if code := do(http.MethodPost, "/api/dirs/csrf", ct, `{}`); code != http.StatusUnsupportedMediaType {
			t.Errorf("POST /api/dirs com Content-Type %q: código %d, esperava 415", ct, code)
		}
	}
	if code := do(http.MethodPost, "/api/notes/ok.md", "application/json; charset=utf-8", `{"content":"x"}`); code != http.StatusCreated {
		t.Errorf("POST JSON: código %d, esperava 201", code)
	}
	if code := do(http.MethodPost, "/api/notes/ok.md", "application/json", `{"content":"y"}`); code != http.StatusConflict {
		t.Errorf("POST JSON repetido: código %d, esperava 409", code)
	}
	if code := do(http.MethodGet, "/api/tree", "", ""); code != http.StatusOK {
		t.Errorf("GET sem Content-Type: código %d, esperava 200", code)
	}
}
