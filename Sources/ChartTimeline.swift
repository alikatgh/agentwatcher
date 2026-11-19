import Foundation

struct ChartTimeline {
    let updated: Date
    let count: Int
    let seconds: TimeInterval

    init(updated: Date, range: Int) {
        self.updated = updated
        count = range == 0 ? 96 : (range == 1 ? 5 : 30)
        seconds = range == 0 ? 900 : 86400
    }

    func bucketIndex(for date: Date) -> Int? {
        let age = updated.timeIntervalSince(date)
        guard age >= 0, age < Double(count) * seconds else { return nil }
        return count - 1 - Int(age / seconds)
    }

    func interval(for index: Int) -> DateInterval {
        let end = updated.addingTimeInterval(-Double(count - 1 - index) * seconds)
        return DateInterval(start: end.addingTimeInterval(-seconds), end: end)
    }

    func label(for index: Int) -> String {
        let interval = interval(for: index)
        let formatter = DateIntervalFormatter()
        formatter.dateStyle = .medium
        formatter.timeStyle = .short
        return formatter.string(from: interval.start, to: interval.end)
    }
}
