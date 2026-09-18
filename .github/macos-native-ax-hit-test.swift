import ApplicationServices
import CoreGraphics
import Darwin
import Foundation

enum HelperError: Error, CustomStringConvertible {
    case message(String)

    var description: String {
        switch self {
        case .message(let value):
            return value
        }
    }
}

struct Frame: Codable {
    let height: Double
    let width: Double
    let x: Double
    let y: Double

    var rect: CGRect {
        CGRect(x: x, y: y, width: width, height: height)
    }
}

struct Point: Codable {
    let x: Double
    let y: Double
}

struct ControlEvidence: Codable {
    let center: Point
    let frame: Frame
    let hitAncestorSubrole: String?
    let hitPid: Int32
    let hitSubrole: String?
    let insideLeftReservedBand: Bool
    let nonOverlapping: Bool
    let subrole: String
}

struct HitTestEvidence: Codable {
    let controls: [ControlEvidence]
    let leftReservedBand: Double
    let nonOverlapping: Bool
    let passed: Bool
    let pid: Int32
    let topChromeBand: Double
    let window: Frame
}

let expectedRoles = ["AXCloseButton", "AXMinimizeButton", "AXZoomButton"]

func attribute(_ element: AXUIElement, _ name: CFString) throws -> CFTypeRef {
    var value: CFTypeRef?
    let error = AXUIElementCopyAttributeValue(element, name, &value)
    guard error == .success, let value else {
        throw HelperError.message("AX attribute unavailable: \(name)")
    }
    return value
}

func stringAttribute(_ element: AXUIElement, _ name: CFString) -> String? {
    (try? attribute(element, name)) as? String
}

func boolAttribute(_ element: AXUIElement, _ name: CFString) -> Bool? {
    (try? attribute(element, name)) as? Bool
}

func elementArray(_ element: AXUIElement, _ name: CFString) throws -> [AXUIElement] {
    guard let values = try attribute(element, name) as? [AXUIElement] else {
        throw HelperError.message("AX attribute is not an element array: \(name)")
    }
    return values
}

func axValueAttribute(_ element: AXUIElement, _ name: CFString) throws -> AXValue {
    let rawValue = try attribute(element, name)
    guard CFGetTypeID(rawValue) == AXValueGetTypeID() else {
        throw HelperError.message("AX attribute is not an AXValue: \(name)")
    }
    return rawValue as! AXValue
}

func pointAttribute(_ element: AXUIElement) throws -> Point {
    let value = try axValueAttribute(element, kAXPositionAttribute as CFString)
    var point = CGPoint.zero
    guard AXValueGetValue(value, .cgPoint, &point) else {
        throw HelperError.message("AX position is unreadable")
    }
    return Point(x: point.x, y: point.y)
}

func sizeAttribute(_ element: AXUIElement) throws -> Point {
    let value = try axValueAttribute(element, kAXSizeAttribute as CFString)
    var size = CGSize.zero
    guard AXValueGetValue(value, .cgSize, &size) else {
        throw HelperError.message("AX size is unreadable")
    }
    return Point(x: size.width, y: size.height)
}

func elementFrame(_ element: AXUIElement) throws -> Frame {
    let position = try pointAttribute(element)
    let size = try sizeAttribute(element)
    return Frame(height: size.y, width: size.x, x: position.x, y: position.y)
}

func controlsUnder(_ element: AXUIElement, depth: Int = 0) throws -> [String: AXUIElement] {
    var controls: [String: AXUIElement] = [:]
    if let subrole = stringAttribute(element, kAXSubroleAttribute as CFString), expectedRoles.contains(subrole) {
        controls[subrole] = element
    }
    guard depth < 12 else {
        return controls
    }
    let children = (try? elementArray(element, kAXChildrenAttribute as CFString)) ?? (try? elementArray(element, kAXContentsAttribute as CFString)) ?? []
    for child in children {
        for (subrole, candidate) in try controlsUnder(child, depth: depth + 1) {
            if controls[subrole] != nil {
                throw HelperError.message("AX exposed duplicate \(subrole) controls")
            }
            controls[subrole] = candidate
        }
    }
    return controls
}

func matchedAncestor(_ element: AXUIElement, expectedSubrole: String) -> (String?, String?) {
    var current: AXUIElement? = element
    for _ in 0..<12 {
        guard let candidate = current else { break }
        let subrole = stringAttribute(candidate, kAXSubroleAttribute as CFString)
        if subrole == expectedSubrole {
            return (subrole, subrole)
        }
        if let rawParent = try? attribute(candidate, kAXParentAttribute as CFString) {
            guard CFGetTypeID(rawParent) == AXUIElementGetTypeID() else { break }
            current = rawParent as! AXUIElement
        } else {
            break
        }
    }
    return (stringAttribute(element, kAXSubroleAttribute as CFString), nil)
}

func hitTest(_ element: AXUIElement, expectedSubrole: String, pid: Int32) throws -> (Int32, String?, String?) {
    let frame = try elementFrame(element)
    let center = CGPoint(x: frame.x + frame.width / 2, y: frame.y + frame.height / 2)
    let systemWide = AXUIElementCreateSystemWide()
    var hit: AXUIElement?
    let error = AXUIElementCopyElementAtPosition(systemWide, Float(center.x), Float(center.y), &hit)
    guard error == .success, let hit else {
        throw HelperError.message("AX screen-position hit test failed for \(expectedSubrole)")
    }
    var hitPid: pid_t = 0
    guard AXUIElementGetPid(hit, &hitPid) == .success, hitPid == pid else {
        throw HelperError.message("AX hit test escaped the owned app for \(expectedSubrole)")
    }
    let (hitSubrole, matchedSubrole) = matchedAncestor(hit, expectedSubrole: expectedSubrole)
    guard matchedSubrole == expectedSubrole else {
        throw HelperError.message("AX hit test did not resolve to \(expectedSubrole)")
    }
    return (hitPid, hitSubrole, matchedSubrole)
}

func fail(_ error: Error) -> Never {
    fputs("macOS AX hit-test failed: \(error)\n", stderr)
    exit(1)
}

guard CommandLine.arguments.count == 4,
      let pid = Int32(CommandLine.arguments[1]),
      let topChromeBand = Double(CommandLine.arguments[2]),
      let leftReservedBand = Double(CommandLine.arguments[3]),
      pid > 0,
      topChromeBand > 0,
      leftReservedBand > 0 else {
    fail(HelperError.message("usage: macos-native-ax-hit-test <pid> <top-band> <left-band>"))
}

do {
    let application = AXUIElementCreateApplication(pid)
    let windows = try elementArray(application, kAXWindowsAttribute as CFString)
    guard let window = windows.first(where: { boolAttribute($0, kAXMainAttribute as CFString) == true }) ?? windows.first else {
        throw HelperError.message("owned app has no AX window")
    }
    let windowFrame = try elementFrame(window)
    guard windowFrame.width > 0, windowFrame.height > 0 else {
        throw HelperError.message("owned AX window has an empty frame")
    }
    let controls = try controlsUnder(window)
    guard expectedRoles.allSatisfy({ controls[$0] != nil }) else {
        throw HelperError.message("owned AX window is missing a required traffic-light role")
    }

    var frames: [String: Frame] = [:]
    for role in expectedRoles {
        guard let control = controls[role] else { throw HelperError.message("missing \(role)") }
        let frame = try elementFrame(control)
        guard frame.width > 0, frame.height > 0 else {
            throw HelperError.message("\(role) has an empty frame")
        }
        guard boolAttribute(control, kAXEnabledAttribute as CFString) == true else {
            throw HelperError.message("\(role) is not enabled")
        }
        let frameRect = frame.rect
        let windowRect = windowFrame.rect
        guard windowRect.contains(frameRect) else {
            throw HelperError.message("\(role) is outside the owned AX window")
        }
        guard frameRect.maxY <= windowFrame.y + topChromeBand,
              frameRect.maxX <= windowFrame.x + leftReservedBand else {
            throw HelperError.message("\(role) is outside the reserved native chrome band")
        }
        frames[role] = frame
    }

    for leftIndex in 0..<expectedRoles.count {
        for rightIndex in (leftIndex + 1)..<expectedRoles.count {
            let left = frames[expectedRoles[leftIndex]]!.rect
            let right = frames[expectedRoles[rightIndex]]!.rect
            guard !left.intersects(right) else {
                throw HelperError.message("traffic-light frames overlap")
            }
        }
    }

    var evidence: [ControlEvidence] = []
    for role in expectedRoles {
        guard let control = controls[role], let frame = frames[role] else {
            throw HelperError.message("missing \(role)")
        }
        let (hitPid, hitSubrole, matchedSubrole) = try hitTest(control, expectedSubrole: role, pid: pid)
        evidence.append(ControlEvidence(
            center: Point(x: frame.x + frame.width / 2, y: frame.y + frame.height / 2),
            frame: frame,
            hitAncestorSubrole: matchedSubrole,
            hitPid: hitPid,
            hitSubrole: hitSubrole,
            insideLeftReservedBand: frame.x >= windowFrame.x && frame.rect.maxX <= windowFrame.x + leftReservedBand,
            nonOverlapping: true,
            subrole: role
        ))
    }

    let output = HitTestEvidence(
        controls: evidence,
        leftReservedBand: leftReservedBand,
        nonOverlapping: true,
        passed: true,
        pid: pid,
        topChromeBand: topChromeBand,
        window: windowFrame
    )
    let encoded = try JSONEncoder().encode(output)
    guard let json = String(data: encoded, encoding: .utf8) else {
        throw HelperError.message("could not encode AX hit-test evidence")
    }
    print(json)
} catch {
    fail(error)
}
