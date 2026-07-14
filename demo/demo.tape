# vhs demo.tape  — https://github.com/charmbracelet/vhs
# Renders demo.gif. Run from this directory: cd demo && vhs demo.tape
#
# Two scenes:
#   1. open demo.log (raw ANSI escapes) and run :ansi-render
#   2. pipe it in — a *stdin* buffer auto-renders, i.e. kakoune as a pager

Output demo.gif

Require kak

Set Shell bash
Set FontSize 20
Set Width 1200
Set Height 600
Set Padding 20
Set Framerate 20
Set TypingSpeed 45ms

# Setup (not recorded): a throwaway config that sources this plugin and
# nothing else, so the demo does not depend on anyone's kakrc. It must load
# at startup — the *stdin* hook has to exist before the buffer is created.
Hide
Type `PS1='$ '` Enter
Type `export XDG_CONFIG_HOME=$(mktemp -d) && mkdir -p $XDG_CONFIG_HOME/kak` Enter
Type `printf 'set-option global startup_info_version 99999999\nsource %s/../rc/ansi.kak\n' "$PWD" > $XDG_CONFIG_HOME/kak/kakrc` Enter
Type "clear" Enter
Sleep 1s
Show

# Scene 1: escapes in the buffer, rendered on demand.
Type "kak demo.log"
Sleep 1s
Enter
Sleep 3.5s

Type ":ansi-render"
Sleep 1s
Enter
Sleep 4s

Hide
Type ":q!" Enter
Sleep 500ms
Type "clear" Enter
Show
Sleep 800ms

# Scene 2: piped in. The *stdin* buffer renders itself — kakoune as a pager.
Type "cat demo.log | kak"
Sleep 1s
Enter
Sleep 5s

Hide
Type ":q!" Enter
