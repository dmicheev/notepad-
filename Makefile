# notepad+ — сборка (Apple Silicon).
# targets:
#   make / make debug  — отладочная сборка (быстрая, dev-профиль)
#   make release       — максимальная оптимизация под текущий M-процессор,
#                        бинарь + .app-бандл (запуск без терминала) в bin/
#   make run           — запуск dev-сборки
#   make test          — все тесты воркспейса
#   make clean         — cargo clean + очистка bin/

BIN_NAME  := notepad-plus
BIN_DIR   := bin
BIN_PATH  := $(BIN_DIR)/$(BIN_NAME)
APP_BUNDLE := $(BIN_DIR)/notepad-plus.app
APP_VERSION := $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
LSREGISTER := /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

# Максимум под хост-M (M1/M2/M3...): код для точной микроархитектуры.
# native на arm64 Apple == текущий чип; на другой машине пересобрать.
RUSTFLAGS_RELEASE := -C target-cpu=native

# Превышаем Cargo.toml-профиль (lto = "thin"): fat-LTO плюс
# один codegen-unit — самый глубокий уровень оптимизации.
CARGO_PROFILE_RELEASE_LTO        = fat
CARGO_PROFILE_RELEASE_CODEGEN_UNITS = 1

.PHONY: all debug release run test clean

all: debug

## Отладочная сборка (dev-профиль; opt-level=3 только для ropey/encoding_rs).
debug:
	cargo build

## Релиз: target-cpu=native + fat LTO + codegen-units=1 → bin/.
## Помимо сырого бинаря собирается notepad-plus.app: LaunchServices
## запускает бандл БЕЗ терминального окна (сырой Mach-O из Finder
## открывается в Terminal).
release:
	RUSTFLAGS="$(RUSTFLAGS_RELEASE)" \
	CARGO_PROFILE_RELEASE_LTO=$(CARGO_PROFILE_RELEASE_LTO) \
	CARGO_PROFILE_RELEASE_CODEGEN_UNITS=$(CARGO_PROFILE_RELEASE_CODEGEN_UNITS) \
	cargo build --release
	@mkdir -p $(BIN_DIR)
	cp target/release/$(BIN_NAME) $(BIN_PATH)
	@# .app-бандл: Contents/{Info.plist, MacOS/<bin>}.
	@mkdir -p "$(APP_BUNDLE)/Contents/MacOS"
	cp packaging/Info.plist "$(APP_BUNDLE)/Contents/Info.plist"
	@sed -i '' 's/VERSION_PLACEHOLDER/$(APP_VERSION)/' "$(APP_BUNDLE)/Contents/Info.plist"
	cp target/release/$(BIN_NAME) "$(APP_BUNDLE)/Contents/MacOS/$(BIN_NAME)"
	@codesign --force --sign - "$(APP_BUNDLE)" 2>/dev/null || true
	@touch "$(APP_BUNDLE)"
	@# Регистрация в LaunchServices: приложение появляется в меню
	@# «Открыть в приложении» (Finder) для поддерживаемых файлов
	@# (packaging/Info.plist → CFBundleDocumentTypes).
	@-$(LSREGISTER) -f "$(APP_BUNDLE)" 2>/dev/null || true
	@ls -lh $(BIN_PATH) "$(APP_BUNDLE)/Contents/MacOS/$(BIN_NAME)"

## Запуск dev-сборки.
run:
	cargo run

## Тесты всего воркспейса.
test:
	cargo test

## Очистка артефактов сборки.
clean:
	cargo clean
	rm -rf $(BIN_DIR)
