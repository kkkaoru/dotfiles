import Foundation
import Testing

@testable import PlamoCore

@Test func cutsAtTheEarliestMarkerWhateverItsOrderInTheList() {
    // The mods send ["<|plamo:tag|>", "<|plamo:bos|>"]: the cut must be whichever appears first in
    // the text, not whichever the list mentions first.
    let text = "translated END <|plamo:tag|> rest"
    #expect(StopMarker.truncate(text, at: ["<|plamo:tag|>", "END"]) == "translated ")
    #expect(StopMarker.truncate(text, at: ["END", "<|plamo:tag|>"]) == "translated ")
}

@Test func keepsTheWholeTextWhenNoMarkerAppears() {
    #expect(StopMarker.truncate("text", at: []) == "text")
    #expect(StopMarker.truncate("text", at: ["", "zzz"]) == "text")
}

@Test func cutsAtTheStartWhenAMarkerIsFirst() {
    #expect(StopMarker.truncate("<|plamo:tag|>text", at: ["<|plamo:tag|>"]).isEmpty)
}
