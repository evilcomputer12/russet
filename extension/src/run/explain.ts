// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// What a compiler error means, in plain words, for the errors Swift developers meet most: the Russet
// Build output's summary and the Problems view add it under the compiler's own message. An error not
// listed here gets none (the compiler's message stands alone).

interface Meaning {
  pattern: RegExp;
  /** `$1`, `$2`, … are the pattern's groups. */
  says: string;
}

const MEANINGS: Meaning[] = [
  {
    pattern: /^cannot find type '([^']+)' in scope/,
    says: "No type named '$1' is visible here. Check the spelling, that it is declared (and not private) in this target, or add the missing `import`.",
  },
  {
    pattern: /^cannot find '([^']+)' in scope/,
    says: "Nothing named '$1' is visible here. Check the spelling, whether it was renamed or removed, that it is not `private` in another file, or add the missing `import`.",
  },
  {
    pattern: /^use of unresolved identifier '([^']+)'/,
    says: "Nothing named '$1' is visible here. Check the spelling, or declare it.",
  },
  {
    pattern: /^no such module '([^']+)'/,
    says: "The module '$1' is not part of this build. Add its package to Package.swift (the package's dependencies and the target's), or check the name.",
  },
  {
    pattern: /^value of (?:optional )?type '([^']+)' has no member '([^']+)'/,
    says: "A value of type $1 has no property or method called '$2'. Check the name, or the value is not of the type you think.",
  },
  {
    pattern: /^value of optional type '([^']+)' must be unwrapped/,
    says: "This value may be nil. Unwrap it first: `if let`, `guard let`, `??` with a default, or `!` if it can never be nil.",
  },
  {
    pattern: /^cannot convert value of type '([^']+)' to (?:expected argument|specified|closure result) type '([^']+)'/,
    says: "A value of type $1 is used where $2 is needed. Convert it, or pass a value of the right type.",
  },
  {
    pattern: /^cannot convert return expression of type '([^']+)' to return type '([^']+)'/,
    says: "This returns a value of type $1, but the function promises $2.",
  },
  {
    pattern: /^missing return in .* expected to return '([^']+)'/,
    says: "Some way through this code ends without returning a value of type $1. Add a `return` on every path.",
  },
  {
    pattern: /^missing argument(?:s)? for parameter(?:s)? (.+) in call/,
    says: "The call leaves out $1, which the function needs. Add it to the call.",
  },
  {
    pattern: /^extra argument(?:s)? (.+) in call/,
    says: "The call passes $1, which the function does not take. Remove it, or check you call the function you mean.",
  },
  {
    pattern: /^incorrect argument label in call \(have '([^']*)', expected '([^']*)'\)/,
    says: "The call's argument labels are ($1), but the function's are ($2).",
  },
  {
    pattern: /^type '([^']+)' does not conform to protocol '([^']+)'/,
    says: "$1 says it is a $2 but lacks something $2 requires (a property or method). Add what is missing.",
  },
  {
    pattern: /^'([^']+)' is inaccessible due to '([^']+)' protection level/,
    says: "'$1' is $2, so this code cannot use it. Make it less private (internal or public), or use it only where it is visible.",
  },
  {
    pattern: /^cannot assign to (?:property|value): '([^']+)' is a 'let' constant/,
    says: "'$1' is a constant (`let`). Declare it with `var` if it should change.",
  },
  {
    pattern: /^cannot assign to property: 'self' is immutable/,
    says: "A struct's method cannot change its properties unless it is `mutating`; in a SwiftUI view, keep changing values in `@State`.",
  },
  {
    pattern: /^left side of mutating operator isn't mutable: '([^']+)' is a 'let' constant/,
    says: "'$1' is a constant (`let`). Declare it with `var` if it should change.",
  },
  {
    pattern: /^'([^']+)' is only available in iOS ([\d.]+) or newer/,
    says: "'$1' needs iOS $2 or newer, but the app also runs on older iOS. Raise the app's minimum iOS (`platforms` in Package.swift), or wrap the use in `if #available(iOS $2, *)`.",
  },
  {
    pattern: /^generic parameter '([^']+)' could not be inferred/,
    says: "Swift cannot tell which type '$1' should be here. Write the type out, e.g. `let x: [Int] = []`.",
  },
  {
    pattern: /^the compiler is unable to type-check this expression in reasonable time/,
    says: "The expression is too big for Swift to work out at once (often a long SwiftUI body). Split it into smaller views or `let`s with explicit types.",
  },
  {
    pattern: /^(?:call to )?main actor-isolated .* (?:in a synchronous nonisolated context|from outside of the actor)/,
    says: "UI code (the main actor) is used from code that may run on another thread. Mark the caller `@MainActor`, or `await` the call from an async context.",
  },
  {
    pattern: /(?:sending|passing) .* risks causing data races/,
    says: "Swift 6 found a value shared between threads that could be changed by both at once. Make it `Sendable` (an immutable struct, an actor), or keep it on one actor.",
  },
  {
    pattern: /^expected '([^']+)'/,
    says: "A syntax slip: Swift expected '$1' here. Look for a missing bracket, parenthesis, comma or quote just before this point.",
  },
  {
    pattern: /^consecutive statements on a line must be separated by ';'/,
    says: "Two statements run together on one line; usually a missing operator, bracket or line break.",
  },
  {
    pattern: /^undefined symbol/,
    says: "Something is declared but its code is not in the app (a source file left out of the target, or a missing library).",
  },
];

/** Names from other UI toolkits (and AI-written code) that SwiftUI spells differently. */
const SWIFTUI_NAMES: Record<string, string> = {
  VBox: "VStack",
  HBox: "HStack",
  ZBox: "ZStack",
  Column: "VStack",
  Row: "HStack",
  Box: "ZStack",
  Stack: "VStack or HStack",
  TextView: "Text (or TextEditor to edit text)",
  EditText: "TextField",
  ImageView: "Image",
  ListView: "List",
  RecyclerView: "List",
  ScrollArea: "ScrollView",
  Padding: "the .padding() modifier",
  Div: "VStack",
};

/** What `message` (the compiler's text, without its location) means in plain words, if it is a common error. */
export function explain(message: string): string | undefined {
  const missing = /^cannot find '([^']+)' in scope/.exec(message.trim())?.[1];
  const swiftui = missing ? SWIFTUI_NAMES[missing] : undefined;
  if (missing && swiftui && swiftui !== missing) {
    return `SwiftUI has no '${missing}': it is called ${swiftui}.`;
  }
  for (const m of MEANINGS) {
    const match = m.pattern.exec(message.trim());
    if (match) {
      return m.says.replace(/\$(\d)/g, (_s, i: string) => match[Number(i)] ?? "");
    }
  }
  return undefined;
}
