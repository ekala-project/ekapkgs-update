module Table exposing (cases)

{-| Table-driven tests: one `test` per `( input, expected )` row, named
after the input (so inputs within a table must be distinct).
-}

import Expect
import Test exposing (Test, test)


cases : String -> (input -> output) -> List ( input, output ) -> Test
cases name f rows =
    Test.describe name
        (List.map
            (\( input, expected ) ->
                test (Debug.toString input) (\_ -> Expect.equal expected (f input))
            )
            rows
        )
