# Records the terminal demo in the README.
#
#   cargo build --release
#   vhs .github/assets/demo.tape      # https://github.com/charmbracelet/vhs
#
# Run it from a directory holding these public corpus files, renamed
# (cargo xtask corpus fetch --only <id>):
#   laminin.lif       aics-s-1-t-1-c-2-z-1 (BSD-3-Clause, Allen Institute for Cell Science)
#   partial-copy.lif  the same file cut to its first 9,000,000 bytes
#   gc-run.ch         entab-test-fid-ch (MIT, entab)

Output demo.gif

Set Shell bash
Set FontSize 14
Set Width 1200
Set Height 500
Set Padding 24
Set Theme "GitHub Dark"
Set TypingSpeed 45ms

Hide
Type "export PS1='$ ' && clear"
Enter
Show

Sleep 500ms
Type "openreadout info laminin.lif"
Sleep 400ms
Enter
Sleep 4s

Type "clear" Enter
Type "openreadout check partial-copy.lif; echo exit=$?"
Sleep 400ms
Enter
Sleep 5s

Type "clear" Enter
Type "openreadout export laminin.lif -o laminin.ome.tiff"
Sleep 400ms
Enter
Sleep 3s

Type "clear" Enter
Type "openreadout analyze peaks gc-run.ch --min-height 1"
Sleep 400ms
Enter
Sleep 4s

Type "clear" Enter
Type "openreadout info laminin.lif --json | jq '.data.images[0].physical_size'"
Sleep 400ms
Enter
Sleep 4s
