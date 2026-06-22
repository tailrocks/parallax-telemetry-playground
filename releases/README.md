# Releases (regression track)

For scenario A13 (deploy + regression): tag `v1` clean and `v2` with an
introduced fault, emit a deploy/release marker + commit sha between them, and
compare how each backend surfaces the regression after the deploy.
