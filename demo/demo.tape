# vhs demo.tape  — https://github.com/charmbracelet/vhs
# Renders demo.gif: opening demo.log (raw ANSI escapes) and running :ansi-render.
# Self-contained: kak -n loads no user config, only this plugin's rc/ansi.kak.
# Run from this directory: cd demo && vhs demo.tape

Output demo.gif

Set Shell bash
Set FontSize 20
Set Width 1200
Set Height 600
Set Padding 20
Set Framerate 20
Set TypingSpeed 45ms

Hide
Type `PS1='$ '`
Enter
Type "clear"
Enter
Sleep 1s
Show

Type "kak -n -e 'source ../rc/ansi.kak' demo.log"
Sleep 1s
Enter
Sleep 4s

Type ":ansi-render"
Sleep 1s
Enter
Sleep 5s

Hide
Type ":q!"
Enter
