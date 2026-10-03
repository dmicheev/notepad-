#!/bin/sh
# Сбор метрик бенчмарк-стенда (design.md D11.1): NOTEPAD_BENCH=1 по
# матрице файлов/кодировок. Результат — stderr приложения (grep по
# [bench]) + пиковая RSS от /usr/bin/time -l.
#
# usage: sh scripts/bench/run.sh [binary] [dir]   > results.txt 2>&1

BIN=${1:-./target/release/notepad-plus}
DIR=${2:-/tmp/notepad-bench}

run() { # run <label> <file>
    label=$1
    file=$2
    if [ ! -f "$file" ]; then
        echo "=== $label MISSING ($file)"
        return
    fi
    echo "=== $label ($(wc -c < "$file" | tr -d ' ') bytes)"
    NOTEPAD_BENCH=1 NOTEPAD_OPEN="$file" /usr/bin/time -l "$BIN" 2>&1 |
        grep -E "^\[bench\]|maximum resident"
}

for size in 1 20 100 200; do
    run "short-${size}mb" "$DIR/short-${size}mb.txt"
    run "cyr-${size}mb" "$DIR/cyr-${size}mb.txt"
done
for prof in long empty code; do
    for size in 20 200; do
        run "$prof-${size}mb" "$DIR/$prof-${size}mb.txt"
    done
done
for size in 20 200; do
    run "short-${size}mb-bom" "$DIR/short-${size}mb.txt.bom"
    run "short-${size}mb-utf16le" "$DIR/short-${size}mb.txt.utf16le"
    run "cyr-${size}mb-cp1251" "$DIR/cyr-${size}mb.txt.cp1251"
done
