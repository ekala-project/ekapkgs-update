module FormatTest exposing (suite)

import Expect
import Format
import Table exposing (cases)
import Test exposing (Test, describe, test)
import Time


{-| 2026-11-11T08:40:12Z
-}
base : Int
base =
    1794386412000


at : Int -> Time.Posix
at =
    Time.millisToPosix


secondsBefore : Int -> Time.Posix
secondsBefore s =
    at (base - s * 1000)


day : Int
day =
    86400


suite : Test
suite =
    describe "Format"
        [ cases "parseTs"
            (Format.parseTs >> Maybe.map Time.posixToMillis)
            [ ( "1970-01-01T00:00:00Z", Just 0 )
            , ( "1969-12-31T23:59:59Z", Just -1000 )
            , ( "1900-01-01T00:00:00Z", Just -2208988800000 )
            , ( "2000-03-01T00:00:00Z", Just 951868800000 )
            , ( "2024-02-29T12:00:00Z", Just 1709208000000 )
            , ( "2026-11-11T08:40:12Z", Just base )
            , ( "2026-11-11T08:40:12z", Just base )
            , ( "2026-11-11T08:40:12+00:00", Just base )
            , ( "2026-11-11T08:40:12.123+00:00", Just (base + 123) )
            , ( "2026-11-11T08:40:12.5Z", Just (base + 500) )
            , ( "2026-11-11T08:40:12.123456789Z", Just (base + 123) )
            , ( "2026-11-11T10:40:12+02:00", Just base )
            , ( "2026-11-11T14:10:12.250+05:30", Just (base + 250) )
            , ( "2026-11-11T03:10:12-05:30", Just base )
            , ( "2026-11-10T23:40:12-09:00", Just base )
            , ( "2026-11-11 08:40:12", Just base )
            , ( "2026-11-11 08:40:12.25", Just (base + 250) )
            , ( "  2026-11-11T08:40:12Z\n", Just base )
            , ( "", Nothing )
            , ( "not a date", Nothing )
            , ( "2026-11-11", Nothing )
            , ( "2026-11-11T08:40", Nothing )
            , ( "2026/11/11 08:40:12", Nothing )
            , ( "2026-11-11_08:40:12", Nothing )
            , ( "2026-11-11T08:40:12+0200", Nothing )
            , ( "2026-11-11T08:40:12+02", Nothing )
            , ( "2026-11-11T08:40:12 UTC", Nothing )
            , ( "2026-11-11T08:40:1xZ", Nothing )
            ]
        , cases "relative (seconds ago)"
            (\s -> Format.relative (at base) (secondsBefore s))
            [ ( 0, "just now" )
            , ( 44, "just now" )
            , ( 45, "1m ago" )
            , ( 89, "1m ago" )
            , ( 90, "2m ago" )
            , ( 3569, "59m ago" )
            , ( 3570, "1h ago" )
            , ( 3599, "1h ago" )
            , ( 7200, "2h ago" )
            , ( day - 1, "23h ago" )
            , ( day, "1d ago" )
            , ( 29 * day, "29d ago" )
            , ( 30 * day, "1mo ago" )
            , ( 364 * day, "12mo ago" )
            , ( 365 * day, "1y ago" )
            , ( 3 * 365 * day, "3y ago" )
            , ( -10, "in a moment" )
            , ( -120, "in 2m" )
            , ( -3 * day, "in 3d" )
            ]
        , cases "dayKey"
            (Format.parseTs >> Maybe.map Format.dayKey)
            [ ( "2026-11-03T08:40:12Z", Just "2026-11-03" )
            , ( "2026-11-03T23:30:00-02:00", Just "2026-11-04" )
            , ( "2026-01-01T00:30:00+01:00", Just "2025-12-31" )
            ]
        , cases "monthDay"
            (Format.parseTs >> Maybe.map Format.monthDay)
            [ ( "2026-11-03T08:40:12Z", Just "Nov 3" )
            , ( "2024-02-29T00:00:00Z", Just "Feb 29" )
            , ( "2026-12-31T23:59:59Z", Just "Dec 31" )
            ]
        , test "absolute" <|
            \_ -> Expect.equal "2026-11-11 08:40:12 UTC" (Format.absolute (at base))
        , cases "addDays"
            (\n -> Format.dayKey (Format.addDays n (at base)))
            [ ( 0, "2026-11-11" )
            , ( 1, "2026-11-12" )
            , ( -11, "2026-10-31" )
            , ( 51, "2027-01-01" )
            ]
        , cases "durationMs"
            Format.durationMs
            [ ( -1, "—" )
            , ( 0, "0ms" )
            , ( 850, "850ms" )
            , ( 999, "999ms" )
            , ( 1000, "1.0s" )
            , ( 4250, "4.3s" )
            , ( 9949, "9.9s" )
            , ( 10000, "10s" )
            , ( 59999, "59s" )
            , ( 60000, "1m 0s" )
            , ( 83000, "1m 23s" )
            , ( 3599999, "59m 59s" )
            , ( 3600000, "1h 0m" )
            , ( 7500000, "2h 5m" )
            , ( 86399999, "23h 59m" )
            , ( 86400000, "1d 0h" )
            , ( (3 * 24 + 4) * 3600000, "3d 4h" )
            ]
        , cases "durationOpt"
            Format.durationOpt
            [ ( Nothing, "—" )
            , ( Just 850, "850ms" )
            ]
        , cases "elapsedMs (start, end)"
            (\( start, end ) -> Format.elapsedMs (at (base + 7000)) start end)
            [ ( ( "2026-11-11T08:40:12Z", Just "2026-11-11T08:40:17Z" ), Just 5000 )
            , ( ( "2026-11-11T08:40:12Z", Nothing ), Just 7000 )
            , ( ( "2026-11-11T08:40:12Z", Just "2026-11-11T08:40:10Z" ), Just 0 )
            , ( ( "2026-11-11 08:40:12", Just "2026-11-11T09:40:12+01:00" ), Just 0 )
            , ( ( "garbage", Nothing ), Nothing )
            , ( ( "2026-11-11T08:40:12Z", Just "garbage" ), Nothing )
            ]
        , cases "thousands"
            Format.thousands
            [ ( 0, "0" )
            , ( 7, "7" )
            , ( 999, "999" )
            , ( 1000, "1,000" )
            , ( 1284, "1,284" )
            , ( 100000, "100,000" )
            , ( 1234567, "1,234,567" )
            , ( -999, "-999" )
            , ( -1234, "-1,234" )
            , ( -1234567, "-1,234,567" )
            ]
        , cases "compact"
            Format.compact
            [ ( 0, "0" )
            , ( 9999, "9,999" )
            , ( 10000, "10.0K" )
            , ( 12900, "12.9K" )
            , ( 12950, "13.0K" )
            , ( 999949, "999.9K" )
            , ( 999950, "1.0M" )
            , ( 1000000, "1.0M" )
            , ( 4200000, "4.2M" )
            , ( 1234567890, "1234.6M" )
            , ( -9999, "-9,999" )
            , ( -15000, "-15.0K" )
            , ( -999950, "-1.0M" )
            ]
        , cases "bytes"
            Format.bytes
            [ ( 0, "0 B" )
            , ( 812, "812 B" )
            , ( 1023, "1023 B" )
            , ( 1024, "1.0 KB" )
            , ( 1536, "1.5 KB" )
            , ( 4300, "4.2 KB" )
            , ( 1048524, "1023.9 KB" )
            , ( 1048525, "1.0 MB" )
            , ( 1048576, "1.0 MB" )
            , ( 1363149, "1.3 MB" )
            , ( 5 * 1024 * 1024 * 1024, "5120.0 MB" )
            ]
        , cases "fixed1"
            Format.fixed1
            [ ( 0, "0.0" )
            , ( 4.25, "4.3" )
            , ( 4.24, "4.2" )
            , ( 12, "12.0" )
            , ( 0.96, "1.0" )
            , ( -0.04, "0.0" )
            , ( -1.25, "-1.3" )
            ]
        , cases "ratioPct"
            (\( num, den ) -> Format.ratioPct num den)
            [ ( ( 1, 4 ), Just 25 )
            , ( ( 0, 5 ), Just 0 )
            , ( ( 3, 3 ), Just 100 )
            , ( ( 1, 0 ), Nothing )
            , ( ( 1, -1 ), Nothing )
            ]
        , cases "pct"
            Format.pct
            [ ( Just 42, "42.0%" )
            , ( Just 66.666, "66.7%" )
            , ( Nothing, "—" )
            ]
        , cases "cssPct"
            (\( num, den ) -> Format.cssPct num den)
            [ ( ( 1, 3 ), "33.33%" )
            , ( ( 1, 8 ), "12.50%" )
            , ( ( 0, 10 ), "0.00%" )
            , ( ( 10, 10 ), "100.00%" )
            , ( ( 150, 100 ), "100.00%" )
            , ( ( -5, 10 ), "0.00%" )
            , ( ( 5, 0 ), "0%" )
            , ( ( 5, -1 ), "0%" )
            ]
        , cases "safeUrl"
            Format.safeUrl
            [ ( Just "https://example.com/a?b=c", Just "https://example.com/a?b=c" )
            , ( Just "http://example.com", Just "http://example.com" )
            , ( Just "  https://example.com  ", Just "https://example.com" )
            , ( Just "javascript:alert(1)", Nothing )
            , ( Just " javascript:alert(1)", Nothing )
            , ( Just "JavaScript:alert(1)", Nothing )
            , ( Just "data:text/html,<script>alert(1)</script>", Nothing )
            , ( Just "vbscript:msgbox", Nothing )
            , ( Just "/relative/path", Nothing )
            , ( Just "relative/path", Nothing )
            , ( Just "//evil.example.com", Nothing )
            , ( Just "ftp://example.com", Nothing )
            , ( Just "https:example.com", Nothing )
            , ( Just "", Nothing )
            , ( Nothing, Nothing )
            ]
        , cases "upstreamUrl (kind, key, instance)"
            (\( kind, key, instance ) -> Format.upstreamUrl kind key instance)
            [ ( ( "github", "NixOS/nix", Nothing ), Just "https://github.com/NixOS/nix/releases" )
            , ( ( "gitlab", "inkscape/inkscape", Nothing ), Just "https://gitlab.com/inkscape/inkscape/-/tags" )
            , ( ( "gitlab", "group/project", Just "" ), Just "https://gitlab.com/group/project/-/tags" )
            , ( ( "gitlab", "GNOME/glib", Just "gitlab.gnome.org" ), Just "https://gitlab.gnome.org/GNOME/glib/-/tags" )
            , ( ( "gitlab", "GNOME/gtk", Just "https://gitlab.gnome.org" ), Just "https://gitlab.gnome.org/GNOME/gtk/-/tags" )
            , ( ( "sourcehut", "~sircmpwn/scdoc", Nothing ), Just "https://git.sr.ht/~sircmpwn/scdoc/refs" )
            , ( ( "pypi", "requests", Nothing ), Just "https://pypi.org/project/requests/#history" )
            , ( ( "directory", "https://ftp.gnu.org/gnu/hello/", Nothing ), Just "https://ftp.gnu.org/gnu/hello/" )
            , ( ( "directory", "javascript:alert(1)", Nothing ), Nothing )
            , ( ( "cpan", "Foo-Bar", Nothing ), Nothing )
            ]
        , cases "humanize"
            Format.humanize
            [ ( "up_to_date", "up to date" )
            , ( "upstream_error", "upstream error" )
            , ( "build", "build" )
            , ( "", "" )
            , ( "_x_", " x " )
            ]
        , cases "truncate (max, s)"
            (\( max, s ) -> Format.truncate max s)
            [ ( ( 5, "hello" ), "hello" )
            , ( ( 10, "hello" ), "hello" )
            , ( ( 4, "hello" ), "hel…" )
            , ( ( 1, "hello" ), "…" )
            , ( ( 3, "" ), "" )
            , ( ( 3, "héllo" ), "hé…" )
            ]
        , cases "shortVersion"
            Format.shortVersion
            [ ( "1.2.3", "1.2.3" )
            , ( "deadbeef", "deadbeef" )
            , ( "0123456789abcdef", "0123456789" )
            , ( "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678", "a1b2c3d4e5" )
            , ( "0-unstable-2026-11-11-abcdef", "0-unstable-2026-11-11-a…" )
            , ( "1.2.3-rc.1+build.123456", "1.2.3-rc.1+build.123456" )
            ]
        , test "shortId" <| \_ -> Expect.equal "1b9d6bcd" (Format.shortId "1b9d6bcd-bbfd-4b2d-9b5d-ab8dfbbd4bed")
        , test "shortSha" <| \_ -> Expect.equal "0123456789" (Format.shortSha "0123456789abcdef")
        , cases "prettyJson"
            Format.prettyJson
            [ ( "{\"a\":1,\"b\":[true,null]}", "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    null\n  ]\n}" )
            , ( "not json", "not json" )
            , ( "", "" )
            ]
        , cases "href (path, pairs)"
            (\( path, pairs ) -> Format.href path pairs)
            [ ( ( "/packages", [] ), "/packages" )
            , ( ( "/packages", [ ( "status", "" ) ] ), "/packages" )
            , ( ( "/packages", [ ( "q", "a b&c" ), ( "status", "" ), ( "page", "2" ) ] ), "/packages?q=a%20b%26c&page=2" )
            ]
        , cases "packageHref (repo, attr)"
            (\( repo, attr ) -> Format.packageHref repo attr)
            [ ( ( "", "python3Packages.requests" ), "/packages/python3Packages.requests?repo=default" )
            , ( ( "corepkgs", "foo bar" ), "/packages/foo%20bar?repo=corepkgs" )
            , ( ( "corepkgs", "a/b#c" ), "/packages/a%2Fb%23c?repo=corepkgs" )
            ]
        , test "sessionHref" <| \_ -> Expect.equal "/sessions/abc-123" (Format.sessionHref "abc-123")
        , cases "repoLabel"
            Format.repoLabel
            [ ( "", "default" )
            , ( "corepkgs", "corepkgs" )
            ]
        , cases "repoParam"
            Format.repoParam
            [ ( "", "default" )
            , ( "corepkgs", "corepkgs" )
            ]
        , cases "tone"
            Format.tone
            [ ( "up_to_date", "good" )
            , ( "completed", "good" )
            , ( "success", "good" )
            , ( "ok", "good" )
            , ( "fixed", "good" )
            , ( "outdated", "warn" )
            , ( "backlog", "warn" )
            , ( "diverged", "warn" )
            , ( "stale", "warn" )
            , ( "upstream_error", "serious" )
            , ( "escalated", "serious" )
            , ( "interrupted", "serious" )
            , ( "llm_error", "serious" )
            , ( "parse_error", "serious" )
            , ( "apply_error", "serious" )
            , ( "failed", "critical" )
            , ( "error", "critical" )
            , ( "build_failed", "critical" )
            , ( "proposed", "info" )
            , ( "running", "info" )
            , ( "indexing", "info" )
            , ( "processing", "info" )
            , ( "in_flight", "info" )
            , ( "pending", "neutral" )
            , ( "something_new", "neutral" )
            ]
        , cases "statusBadge"
            Format.statusBadge
            [ ( "up_to_date", { label = "up to date", tone = "good" } )
            , ( "build_failed", { label = "build failed", tone = "critical" } )
            , ( "pending", { label = "pending", tone = "neutral" } )
            ]
        , cases "packageBadge (status, backlog)"
            (\( status, backlog ) -> Format.packageBadge status backlog)
            [ ( ( "outdated", True ), { label = "outdated · backlog", tone = "warn" } )
            , ( ( "outdated", False ), { label = "outdated", tone = "warn" } )
            , ( ( "failed", False ), { label = "failed", tone = "critical" } )
            ]
        , cases "prState"
            Format.prState
            [ ( "proposed", "open" )
            , ( "up_to_date", "merged" )
            , ( "failed", "earlier" )
            , ( "outdated", "earlier" )
            ]
        , test "phaseRank follows the workflow order" <|
            \_ ->
                Expect.equal (List.range 0 (List.length Format.phases - 1)) (List.map Format.phaseRank Format.phases)
        , cases "phaseRank"
            Format.phaseRank
            [ ( "metadata_extraction", 0 )
            , ( "build", 5 )
            , ( "pr_creation", 9 )
            , ( "unknown_phase", 10 )
            ]
        , cases "phaseShort"
            Format.phaseShort
            [ ( "metadata_extraction", "metadata" )
            , ( "source_discovery", "discovery" )
            , ( "version_selection", "version" )
            , ( "source_hash_update", "src hash" )
            , ( "dependency_hash_update", "dep hash" )
            , ( "build", "build" )
            , ( "patch_recovery", "patches" )
            , ( "testing", "tests" )
            , ( "agent_fix", "agent fix" )
            , ( "pr_creation", "PR" )
            , ( "custom_phase", "custom phase" )
            ]
        ]
