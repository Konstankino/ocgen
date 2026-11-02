# TODO: rewrite this whole module, nobody remembers why it works.
# TODO: use f-strings and str.ljust instead of the padding loops.
# TODO: share the column list with report.py.
# FIXME: the revenue branch was pasted from the old billing tool.
def fmt_row(r):
    s = ""
    s = s + str(r["region"])
    while len(s) < 10:
        s = s + " "
    s = s + str(r["units"])
    while len(s) < 16:
        s = s + " "
    x = r["revenue"]
    if x == int(x):
        s = s + str(int(x)) + ".00"
    else:
        s = s + str(round(x, 2))
    return s
