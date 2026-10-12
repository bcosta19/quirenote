// Package config reads and writes the user configuration
// (~/.config/quirenote/config.toml), which records the chosen vault.
package config

import (
	"bufio"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

type Config struct {
	Vault string // absolute path of the vault; empty = not configured yet
}

// Path returns the config file location ($XDG_CONFIG_HOME/quirenote/config.toml).
func Path() (string, error) {
	dir, err := os.UserConfigDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, "quirenote", "config.toml"), nil
}

// Load reads the config file. A missing file is not an error: it returns an empty Config.
// Only the subset of TOML the app writes is understood: `key = "string"` lines and comments.
func Load(path string) (Config, error) {
	var c Config
	f, err := os.Open(path)
	if errors.Is(err, fs.ErrNotExist) {
		return c, nil
	}
	if err != nil {
		return c, err
	}
	defer func() { _ = f.Close() }()
	sc := bufio.NewScanner(f)
	for n := 1; sc.Scan(); n++ {
		line := strings.TrimSpace(sc.Text())
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		key, val, ok := strings.Cut(line, "=")
		if !ok {
			return c, fmt.Errorf("%s:%d: expected key = \"value\"", path, n)
		}
		s, err := strconv.Unquote(strings.TrimSpace(val))
		if err != nil {
			return c, fmt.Errorf("%s:%d: the value must be a quoted string", path, n)
		}
		if strings.TrimSpace(key) == "vault" {
			c.Vault = s
		}
	}
	return c, sc.Err()
}

// Save writes the config file atomically, creating its directory.
func Save(path string, c Config) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	data := "# Quirenote configuration\nvault = " + strconv.Quote(c.Vault) + "\n"
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, []byte(data), 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}
