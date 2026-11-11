import Foundation

@main
enum StartupUpdaterTests {
    static func main() throws {
        if try StartupUpdaterVerification.respondToChildInvocation() { return }
        try StartupUpdaterVerification.runTests()
    }
}
