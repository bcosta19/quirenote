VAULT ?= $(HOME)/Documents/NotasObsidian/Notas
ADDR  ?= 127.0.0.1:8080

.PHONY: dev dev-api dev-web build front check install clean

# Em dev: Go em :8080 + Vite (hot reload) em :5173, que repassa /api para o Go.
dev:
	@$(MAKE) -j2 dev-api dev-web

dev-api:
	./scripts/dev-api.sh -vault "$(VAULT)" -addr $(ADDR) -debug

dev-web:
	cd web && npm run dev

front:
	cd web && npm ci && npm run build
	@touch web/dist/.keep

build: front
	go build -trimpath -ldflags="-s -w" -o bin/notas ./cmd/notas

check:
	gofmt -l . | (! grep .)
	go vet ./...
	go run honnef.co/go/tools/cmd/staticcheck@latest ./...
	go run github.com/kisielk/errcheck@latest ./...
	go run go.uber.org/nilaway/cmd/nilaway@latest ./...
	go test -race ./...
	cd web && npm run check

install: build
	install -Dm755 bin/notas $(HOME)/.local/bin/notas
	install -Dm644 deploy/notas.service $(HOME)/.config/systemd/user/notas.service
	install -Dm755 deploy/notas-app $(HOME)/.local/bin/notas-app
	install -Dm644 web/public/icon-512.png $(HOME)/.local/share/icons/hicolor/512x512/apps/notas.png
	# Caminho absoluto no Icon=: o launcher acha mesmo sem cache de ícones.
	sed 's|^Icon=.*|Icon=$(HOME)/.local/share/icons/hicolor/512x512/apps/notas.png|' deploy/notas.desktop > $(HOME)/.local/share/applications/notas.desktop
	systemctl --user daemon-reload
	@echo "Pronto. Ative com: systemctl --user enable --now notas (o atalho 'Notas' já está no launcher)"

clean:
	rm -rf bin web/dist/assets web/dist/index.html web/dist/icon.svg web/dist/icon-512.png web/dist/manifest.webmanifest
