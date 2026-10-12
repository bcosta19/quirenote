package config

import (
	"os"
	"path/filepath"
	"testing"
)

func TestRoundTrip(t *testing.T) {
	p := filepath.Join(t.TempDir(), "sub", "config.toml")
	c, err := Load(p)
	if err != nil || c.Vault != "" {
		t.Fatalf("missing file: %+v, %v", c, err)
	}
	want := `/home/x/My "Notes"`
	if err := Save(p, Config{Vault: want}); err != nil {
		t.Fatal(err)
	}
	if c, err = Load(p); err != nil || c.Vault != want {
		t.Fatalf("Load = %+v, %v", c, err)
	}
}

func TestLoadInvalid(t *testing.T) {
	p := filepath.Join(t.TempDir(), "config.toml")
	for _, bad := range []string{"vault /x\n", "vault = /x\n"} {
		if err := os.WriteFile(p, []byte(bad), 0o644); err != nil {
			t.Fatal(err)
		}
		if _, err := Load(p); err == nil {
			t.Errorf("Load(%q) should fail", bad)
		}
	}
}
