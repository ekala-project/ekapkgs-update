port module TestRunner exposing (program)

{-| A minimal headless runner for elm-explorations/test.

`run.sh` generates a `TestMain` that passes every `suite` in `tests/` to
`program`, compiles it with `elm make` and runs it with QuickJS
(`driver.js`). The driver sends a seed through `start`; the runner runs
every test synchronously and replies on `report` with the text to print
and the number of failures.

-}

import Random
import Test exposing (Test)
import Test.Runner exposing (Runner, SeededRunners(..))
import Test.Runner.Failure as Failure exposing (InvalidReason(..), Reason)


port start : (Int -> msg) -> Sub msg


port report : { text : String, failed : Int } -> Cmd msg


{-| Fuzz tests (if any) run this many times.
-}
fuzzRuns : Int
fuzzRuns =
    100


program : List Test -> Program () () Int
program suites =
    Platform.worker
        { init = \() -> ( (), Cmd.none )
        , update = \seed model -> ( model, report (runAll seed (Test.describe "frontend" suites)) )
        , subscriptions = \_ -> start identity
        }


type Outcome
    = Passed
    | Failed String
    | Todo


runAll : Int -> Test -> { text : String, failed : Int }
runAll seed test =
    let
        summarize : String -> List Runner -> { text : String, failed : Int }
        summarize mode runners =
            let
                outcomes =
                    List.concatMap runOne runners

                failures =
                    List.filterMap
                        (\o ->
                            case o of
                                Failed msg ->
                                    Just msg

                                _ ->
                                    Nothing
                        )
                        outcomes

                count p =
                    List.length (List.filter p outcomes)

                passed =
                    count ((==) Passed)

                todos =
                    count ((==) Todo)

                -- `only` and `skip` must not slip into a passing run.
                modeFailure =
                    if mode == "" then
                        0

                    else
                        1
            in
            { text =
                String.join "\n\n" failures
                    ++ (if List.isEmpty failures then
                            ""

                        else
                            "\n\n"
                       )
                    ++ String.join "\n"
                        (List.filter (not << String.isEmpty)
                            [ mode
                            , "Seed: " ++ String.fromInt seed
                            , "Passed: " ++ String.fromInt passed
                            , "Failed: " ++ String.fromInt (List.length failures)
                            , if todos > 0 then
                                "Todo: " ++ String.fromInt todos

                              else
                                ""
                            ]
                        )
            , failed = List.length failures + todos + modeFailure
            }
    in
    case Test.Runner.fromTest fuzzRuns (Random.initialSeed seed) test of
        Plain runners ->
            summarize "" runners

        Only runners ->
            summarize "Test.only is in use; other tests did not run." runners

        Skipping runners ->
            summarize "Test.skip is in use; some tests did not run." runners

        Invalid reason ->
            { text = "Invalid test suite: " ++ reason, failed = 1 }


runOne : Runner -> List Outcome
runOne runner =
    let
        label =
            String.join " > " (List.reverse runner.labels)
    in
    runner.run ()
        |> List.map
            (\expectation ->
                if Test.Runner.isTodo expectation then
                    Todo

                else
                    case Test.Runner.getFailureReason expectation of
                        Nothing ->
                            Passed

                        Just failure ->
                            Failed
                                ("FAIL "
                                    ++ label
                                    ++ (case failure.given of
                                            Just given ->
                                                "\n  given: " ++ given

                                            Nothing ->
                                                ""
                                       )
                                    ++ "\n"
                                    ++ indent (describe failure.description failure.reason)
                                )
            )


describe : String -> Reason -> String
describe description reason =
    case reason of
        Failure.Custom ->
            description

        Failure.Equality expected actual ->
            "expected: " ++ expected ++ "\n  actual: " ++ actual ++ "\n(" ++ description ++ ")"

        Failure.Comparison first second ->
            first ++ "\n" ++ description ++ "\n" ++ second

        Failure.ListDiff expected actual ->
            "expected: [" ++ String.join ", " expected ++ "]\n  actual: [" ++ String.join ", " actual ++ "]"

        Failure.CollectionDiff diff ->
            "expected: "
                ++ diff.expected
                ++ "\n  actual: "
                ++ diff.actual
                ++ "\n   extra: "
                ++ String.join ", " diff.extra
                ++ "\n missing: "
                ++ String.join ", " diff.missing

        Failure.TODO ->
            "TODO: " ++ description

        Failure.Invalid invalid ->
            "invalid test (" ++ invalidReason invalid ++ "): " ++ description


invalidReason : InvalidReason -> String
invalidReason invalid =
    case invalid of
        EmptyList ->
            "empty list"

        NonpositiveFuzzCount ->
            "non-positive fuzz count"

        InvalidFuzzer ->
            "invalid fuzzer"

        BadDescription ->
            "bad description"

        DuplicatedName ->
            "duplicated name"

        DistributionInsufficient ->
            "distribution insufficient"

        DistributionBug ->
            "distribution bug"


indent : String -> String
indent =
    String.lines >> List.map (\l -> "    " ++ l) >> String.join "\n"
