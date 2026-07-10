# kak-ansi-rs: render ANSI-colored text in Kakoune.
# Drop-in replacement for kak-ansi; same options, commands, hooks and
# calling conventions, with the filter binary written in Rust.

declare-option -hidden range-specs ansi_color_ranges
declare-option -hidden str ansi_command_file
declare-option -hidden str ansi_filter %sh{
    plugindir="$(dirname "$kak_source")/.."
    filter=''
    for candidate in \
        "${plugindir}/target/release/kak-ansi-filter" \
        "${plugindir}/kak-ansi-filter"
    do
        if [ -x "$candidate" ]; then
            filter=$candidate
            break
        fi
    done
    if [ -z "$filter" ]; then
        filter=$(command -v kak-ansi-filter)
    fi
    if [ -z "$filter" ] && command -v cargo >/dev/null 2>&1 && [ -f "${plugindir}/Cargo.toml" ]; then
        echo "kak-ansi: building kak-ansi-filter with cargo" >&2
        ( cd "$plugindir" && cargo build --release >&2 )
        if [ -x "${plugindir}/target/release/kak-ansi-filter" ]; then
            filter="${plugindir}/target/release/kak-ansi-filter"
        fi
    fi
    # the path is single-quoted when interpolated into the pipe register;
    # refuse paths that would break out of the quoting
    case "$filter" in
        *"'"*) filter='' ;;
    esac
    if [ -z "$filter" ]; then
        echo "kak-ansi: kak-ansi-filter not found, falling back to cat (no colors)" >&2
        filter=$(command -v cat)
    fi
    printf '%s' "$filter"
}

define-command \
    -docstring %{ansi-render-selection: colorize ANSI codes contained inside selection

After highlighters are added to colorize the buffer, the ANSI codes
are removed.} \
    -params 0 \
    ansi-render-selection %{
    try ansi-setup-buffer
    ansi-render-selection-impl
}
define-command -hidden ansi-setup-buffer %{
    add-highlighter buffer/ansi ranges ansi_color_ranges
    set-option buffer ansi_color_ranges %val{timestamp}
    # single-quoted when interpolated into the pipe register (like the
    # filter path); refuse paths that would break out of the quoting
    set-option buffer ansi_command_file %sh{
        f=$(mktemp)
        case "$f" in
            *"'"*) rm -f "$f"; f=/dev/null ;;
        esac
        printf %s "$f"
    }
    hook -always -once buffer BufClose .* %{
	    nop %sh{rm -f "${kak_opt_ansi_command_file}"}
	    set-option buffer ansi_command_file /dev/null
    }
}

define-command -hidden ansi-render-selection-impl %{
    evaluate-commands -save-regs | %{
        set-register '|' "'%opt{ansi_filter}' -range %val{selection_desc} 2>'%opt{ansi_command_file}'"
        execute-keys "|<ret>"
        update-option buffer ansi_color_ranges
        source "%opt{ansi_command_file}"
        trigger-user-hook "AnsiColored=%val(selection_desc)"
    }
}

define-command \
    -docstring %{ansi-render: colorize buffer by using ANSI codes  After highlighters are added to colorize the buffer, the ANSI codes are removed.} \
    -params 0 \
    ansi-render %{
    evaluate-commands -draft %{
        execute-keys '%'
        ansi-render-selection
    }
}

define-command \
    -docstring %{ansi-clear: clear highlighting for current buffer.} \
    -params 0 \
    ansi-clear %{
    set-option buffer ansi_color_ranges %val{timestamp}
}

define-command \
    -docstring %{ansi-enable: start rendering new fifo data in current buffer.} \
    -params 0 \
    ansi-enable %{
    try ansi-setup-buffer
    ansi-render
    remove-hooks buffer ansi
    hook -group ansi buffer BufReadFifo .* %{
        evaluate-commands -draft %{
            select "%val{hook_param}"
            ansi-render-selection-impl
        }
    }
}

define-command \
    -docstring %{ansi-disable: stop rendering new fifo content in current buffer.} \
    -params 0 \
    ansi-disable %{
        remove-hooks buffer ansi
    }

hook -group ansi global BufCreate '\*stdin(?:-\d+)?\*' ansi-enable

hook -once -group ansi global KakBegin '.*' %{
    define-command -override -hidden -params ..3 man-impl %{ evaluate-commands %sh{
        buffer_name="$1"
        if [ -z "${buffer_name}" ]; then
            exit
        fi
        shift
        manout=$(mktemp "${TMPDIR:-/tmp}"/kak-man.XXXXXX)
        manerr=$(mktemp "${TMPDIR:-/tmp}"/kak-man.XXXXXX)
        env MAN_KEEP_FORMATTING=1 MANWIDTH=${kak_window_range##* } man "$@" > "$manout" 2> "$manerr"
        retval=$?

        if [ "${retval}" -eq 0 ]; then
            printf %s\\n "
                    edit -scratch %{*$buffer_name ${*}*}
                    execute-keys '%|cat<space>${manout}<ret>gk'
                    ansi-enable
                    nop %sh{ rm -f \"${manout}\" \"${manerr}\" }
                    set-option buffer filetype man
                    set-option window manpage $buffer_name $*
            "
        else
            printf '
                fail %%{%s}
                nop %%sh{ rm -f "%s" "%s" }
            ' "$(cat "$manerr")" "${manout}" "${manerr}"
        fi
    } }
}
