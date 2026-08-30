# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""`python3 -m acheron_gui` entry point — mirrors `gui/main.py`.

The installed `acheron-gui` launcher (see `packaging/acheron-gui` and
`install.sh`) points `PYTHONPATH` at the installed copy of this package and
runs it with `-m acheron_gui`, so the desktop launch path never depends on
a git checkout or on `gui/main.py` being on disk.
"""

from acheron_gui.app import main

if __name__ == "__main__":
    main()
