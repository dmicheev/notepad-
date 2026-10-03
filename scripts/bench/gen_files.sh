#!/bin/sh
# Генерация бенчмарк-файлов (design.md D11.1).
#
# Профили:
#   short  — короткие строки (~70 Б), паттерн /tmp/notepad-bench-20mb.txt
#   long   — длинные строки ~1 КБ
#   empty  — короткие вперемешку с пустыми (каждая третья пустая)
#   cyr    — UTF-8 с кириллицей (2-байтовые кодовые точки)
#   code   — вложенные отступы (профиль для fold-замеров: тела блоков)
#
# Размеры: профили short/cyr — 1/20/100/200 МБ; long/empty/code — 20/200 МБ
# (экономия диска). Кодировочные варианты (UTF-8+BOM, UTF-16LE+BOM,
# windows-1251) — 20 и 200 МБ. Целевой размер — в байтах, ± одна строка.
#
# usage: sh scripts/bench/gen_files.sh [dir]   (по умолчанию /tmp/notepad-bench)

set -e
DIR=${1:-/tmp/notepad-bench}
mkdir -p "$DIR"

gen() { # gen <profile> <bytes> <out>
    profile=$1; target=$2; out=$3
    [ -f "$out" ] && { echo "skip $out (exists)"; return; }
    LC_ALL=C awk -v target="$target" -v prof="$profile" 'BEGIN{
        n = 0; bytes = 0;
        while (bytes < target) {
            if (prof == "short") {
                line = sprintf("line %06d — the quick brown fox jumps over the lazy dog 0123456789", n)
            } else if (prof == "long") {
                s = ""; w = 0
                while (length(s) < 1000) { s = s sprintf("word%05d_%06d ", w, n * 31 + w); w++ }
                line = s
            } else if (prof == "empty") {
                if (n % 3 == 1) line = ""; else line = sprintf("row %06d some ascii text here", n)
            } else if (prof == "cyr") {
                line = sprintf("строка %06d — съешь ещё этих мягких французских булок, да выпей чаю", n)
            } else if (prof == "code") {
                d = n % 9; ind = ""
                for (i = 0; i < d; i++) ind = ind "    "
                line = ind sprintf("call_%06d(arg1, arg2) { if (deep) { trace(); } }", n)
            }
            printf "%s\n", line
            bytes += length(line) + 1
            n++
        }
    }' > "$out"
    echo "$out: $(wc -c < "$out" | tr -d ' ') bytes"
}

MB=1048576
for prof in short cyr; do
    for size in 1 20 100 200; do
        gen "$prof" $((size * MB)) "$DIR/$prof-${size}mb.txt"
    done
done
for prof in long empty code; do
    for size in 20 200; do
        gen "$prof" $((size * MB)) "$DIR/$prof-${size}mb.txt"
    done
done

# Кодировочные варианты (матрица открытия D11.2): BOM-подпись/UTF-16LE
# кодируются вручную (encoding_rs не даёт UTF-16 энкодера), windows-1251 —
# iconv из кириллического профиля.
for size in 20 200; do
    f="$DIR/short-${size}mb.txt"
    [ -f "$f.bom" ] || { printf '\357\273\277' | cat - "$f" > "$f.bom"; echo "$f.bom: $(wc -c < "$f.bom" | tr -d ' ') bytes"; }
    [ -f "$f.utf16le" ] || { iconv -f UTF-8 -t UTF-16LE "$f" > "$f.utf16le.tmp"; printf '\377\376' | cat - "$f.utf16le.tmp" > "$f.utf16le"; rm "$f.utf16le.tmp"; echo "$f.utf16le: $(wc -c < "$f.utf16le" | tr -d ' ') bytes"; }
    c="$DIR/cyr-${size}mb.txt"
    [ -f "$c.cp1251" ] || { iconv -f UTF-8 -t WINDOWS-1251 "$c" > "$c.cp1251"; echo "$c.cp1251: $(wc -c < "$c.cp1251" | tr -d ' ') bytes"; }
done
