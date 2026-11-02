# Sourced by the runners and the release smoke (bash and zsh): the shipped mode file's spans, read
# from this checkout's `assets/modes/battle.ron`, the file the server compiles in
# (`BattleMode::shipped`). Every scripted client takes its input slot during the fill-up and plays
# its script only from combat release (`net::harness::buffer_input`), so a runner's wall-clock
# bound on a client derives from the fill-up it waits out plus the script it plays.

# `battle_fill_up_s <repo>`: the fill-up in whole seconds, rounded up, or a failing status with
# its reason on stderr when the field cannot be read.
battle_fill_up_s() {
    local mode="$1/assets/modes/battle.ron"
    awk '
        /^[[:space:]]*fill_up_s[[:space:]]*:/ {
            span = $0
            sub(/^[[:space:]]*fill_up_s[[:space:]]*:[[:space:]]*/, "", span)
            sub(/[[:space:]]*\/\/.*$/, "", span)
            sub(/,[[:space:]]*$/, "", span)
            sub(/[[:space:]]*$/, "", span)
            # Accept only a whole RON number; awk numeric coercion alone turns junk into zero.
            if (span !~ /^[+]?[0-9]+([.][0-9]+)?([eE][+-]?[0-9]+)?$/) exit 1
            span = span + 0
            print int(span) + (span > int(span)); found = 1; exit
        }
        END { if (!found) exit 1 }
    ' "$mode" || { echo "cannot read fill_up_s from $mode" >&2; return 1; }
}
