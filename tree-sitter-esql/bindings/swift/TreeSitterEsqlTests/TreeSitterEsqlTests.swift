import XCTest
import SwiftTreeSitter
import TreeSitterEsql

final class TreeSitterEsqlTests: XCTestCase {
    func testCanLoadGrammar() throws {
        let parser = Parser()
        let language = Language(language: tree_sitter_esql())
        XCTAssertNoThrow(try parser.setLanguage(language),
                         "Error loading ESQL grammar")
    }
}
