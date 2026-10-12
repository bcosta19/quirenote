package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"quirenote/internal/api"
	"quirenote/internal/config"
	"quirenote/internal/setup"
	"quirenote/internal/vault"
	"quirenote/internal/watch"
	"quirenote/web"
)

const usage = `Usage:
  quirenote [-vault DIR] [-addr HOST:PORT] [-debug]   start the server
  quirenote init [DIR]                                 choose the vault (shows it without DIR)

The vault comes from -vault, then QUIRENOTE_VAULT, then the config file
(%s). Without any of them, the browser shows a first-run screen
to start a new vault or use an existing folder (an Obsidian vault works as is).

Flags:
`

func main() {
	cfgPath, err := config.Path()
	if err != nil {
		fatal(err)
	}
	if len(os.Args) > 1 && os.Args[1] == "init" {
		if err := initVault(cfgPath, os.Args[2:]); err != nil {
			fatal(err)
		}
		return
	}
	flag.Usage = func() {
		_, _ = fmt.Fprintf(flag.CommandLine.Output(), usage, cfgPath)
		flag.PrintDefaults()
	}
	addr := flag.String("addr", env("QUIRENOTE_ADDR", env("NOTAS_ADDR", "127.0.0.1:8080")), "HTTP address (use the NetBird/Tailscale IP for remote access)")
	dir := flag.String("vault", env("QUIRENOTE_VAULT", env("NOTAS_VAULT", "")), "vault directory (overrides the config file)")
	debug := flag.Bool("debug", false, "verbose logging")
	flag.Parse()

	if *debug {
		slog.SetLogLoggerLevel(slog.LevelDebug)
	}
	if *dir == "" {
		cfg, err := config.Load(cfgPath)
		if err != nil {
			fatal(err)
		}
		*dir = cfg.Vault
	}
	if err := run(*addr, *dir, cfgPath); err != nil {
		fatal(err)
	}
}

// initVault implements `quirenote init [DIR]`: an existing folder is used as is,
// a missing one is created with a welcome note.
func initVault(cfgPath string, args []string) error {
	cfg, err := config.Load(cfgPath)
	if err != nil {
		return err
	}
	if len(args) == 0 {
		if cfg.Vault == "" {
			fmt.Printf("No vault configured. Run: quirenote init <folder>\n")
		} else {
			fmt.Printf("Vault: %s\nConfig: %s\n", cfg.Vault, cfgPath)
		}
		return nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return err
	}
	res, err := setup.Auto(args[0], home)
	if err != nil {
		return err
	}
	if err := config.Save(cfgPath, config.Config{Vault: res.Dir}); err != nil {
		return err
	}
	fmt.Printf("Vault: %s\n", res.Dir)
	if res.Obsidian {
		fmt.Println("Obsidian vault detected: your notes keep working in Obsidian.")
	}
	fmt.Printf("Saved to %s. Restart the server to use it (systemctl --user restart quirenote).\n", cfgPath)
	return nil
}

func run(addr, dir, cfgPath string) error {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	broker := watch.NewBroker()
	serve := func(v *vault.Vault) {
		go func() {
			if err := watch.Run(ctx, v.Dir(), broker); err != nil {
				slog.Error("watch", "err", err)
			}
		}()
	}

	var v *vault.Vault
	if dir != "" {
		var err error
		if v, err = vault.Open(dir); err != nil {
			return fmt.Errorf("opening the vault: %w (choose another one with `quirenote init <folder>`)", err)
		}
		defer func() { _ = v.Close() }()
		serve(v)
	}

	static := web.Dist()
	if static == nil {
		slog.Warn("frontend not embedded: run `make build` or use `make dev` (Vite)")
	}
	srv := api.New(v, broker, static)
	if v == nil {
		home, err := os.UserHomeDir()
		if err != nil {
			return err
		}
		srv.EnableSetup(home, func(res setup.Result) error {
			nv, err := vault.Open(res.Dir)
			if err != nil {
				return err
			}
			if err := config.Save(cfgPath, config.Config{Vault: res.Dir}); err != nil {
				_ = nv.Close()
				return err
			}
			serve(nv)
			srv.SetVault(nv)
			slog.Info("quirenote", "vault", res.Dir, "config", cfgPath)
			return nil
		})
		slog.Info("quirenote: no vault configured; open the app to choose one", "config", cfgPath)
	}

	hs := &http.Server{
		Addr:              addr,
		Handler:           srv.Handler(),
		ReadHeaderTimeout: 10 * time.Second,
		BaseContext:       func(net.Listener) context.Context { return ctx },
	}
	go func() {
		<-ctx.Done()
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = hs.Shutdown(shutdownCtx)
	}()

	slog.Info("quirenote", "addr", "http://"+addr, "vault", dir)
	if err := hs.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
		return err
	}
	return nil
}

func fatal(err error) {
	slog.Error("quirenote", "err", err)
	os.Exit(1)
}

// env reads an environment variable (NOTAS_* are the old names, still accepted).
func env(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}
