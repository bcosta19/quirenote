package main

import (
	"context"
	"errors"
	"flag"
	"log/slog"
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"
	"time"

	"quirenote/internal/api"
	"quirenote/internal/vault"
	"quirenote/internal/watch"
	"quirenote/web"
)

func main() {
	home, _ := os.UserHomeDir()
	addr := flag.String("addr", env("QUIRENOTE_ADDR", env("NOTAS_ADDR", "127.0.0.1:8080")), "endereço HTTP (use o IP do NetBird/Tailscale para acesso remoto)")
	dir := flag.String("vault", env("QUIRENOTE_VAULT", env("NOTAS_VAULT", filepath.Join(home, "Documents/NotasObsidian/Notas"))), "diretório do vault")
	debug := flag.Bool("debug", false, "log detalhado")
	flag.Parse()

	if *debug {
		slog.SetLogLoggerLevel(slog.LevelDebug)
	}
	if err := run(*addr, *dir); err != nil {
		slog.Error("quirenote", "err", err)
		os.Exit(1)
	}
}

func run(addr, dir string) error {
	v, err := vault.Open(dir)
	if err != nil {
		return err
	}
	defer func() { _ = v.Close() }()

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	broker := watch.NewBroker()
	go func() {
		if err := watch.Run(ctx, dir, broker); err != nil {
			slog.Error("watch", "err", err)
		}
	}()

	static := web.Dist()
	if static == nil {
		slog.Warn("front não embutido: rode `make build` ou use `make dev` (Vite)")
	}

	srv := &http.Server{
		Addr:              addr,
		Handler:           api.New(v, broker, static).Handler(),
		ReadHeaderTimeout: 10 * time.Second,
		BaseContext:       func(net.Listener) context.Context { return ctx },
	}
	go func() {
		<-ctx.Done()
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = srv.Shutdown(shutdownCtx)
	}()

	slog.Info("quirenote", "addr", "http://"+addr, "vault", dir)
	if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
		return err
	}
	return nil
}

// env lê uma variável de ambiente (NOTAS_* são os nomes antigos, ainda aceitos).
func env(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}
