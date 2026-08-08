// A ground-truth oracle for `rewo_world::suggestions` (M114).
//
// WHY THIS EXISTS. `com.mojang.brigadier` is a library, so it is absent from
// the decompiled client tree every other Rewo milestone transcribes from. The
// jar is on disk (Phase B downloads it), so rather than transcribe from memory
// this runs the REAL classes and prints what they produce. The vectors it
// emits are pasted into `suggestions.rs`'s tests as pinned expectations —
// M12's star-field and M14's tint tables use the same pattern.
//
// It also grades `String.compareToIgnoreCase`, which is the one piece of the
// port that is a JDK behaviour rather than a brigadier one, and which is easy
// to get backwards: the fold is upper-then-lower, and `_` sorts differently
// under each.
//
// RUN (from the repo root):
//   $JDK/bin/java -cp "<brigadier.jar>" tools/suggestion_oracle/Oracle.java
// See `run.ps1` in this directory for the resolved paths.

import com.mojang.brigadier.context.StringRange;
import com.mojang.brigadier.suggestion.Suggestion;
import com.mojang.brigadier.suggestion.Suggestions;
import com.mojang.brigadier.suggestion.SuggestionsBuilder;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

public final class Oracle {
   /** One `SuggestionsBuilder` case: input, start, and the texts offered. */
   private static void builderCase(final String label, final String input, final int start, final String... texts) {
      SuggestionsBuilder builder = new SuggestionsBuilder(input, start);
      for (String text : texts) {
         builder.suggest(text);
      }
      Suggestions built = builder.build();
      StringBuilder out = new StringBuilder();
      out.append("BUILD\t").append(label).append('\t').append(built.getRange().getStart()).append('\t').append(built.getRange().getEnd());
      for (Suggestion suggestion : built.getList()) {
         out.append('\t').append(suggestion.getText());
      }
      System.out.println(out);
   }

   /** `Suggestions.merge` of two single-entry sets with different ranges. */
   private static void mergeCase(final String label, final String command, final int aStart, final int aEnd, final String aText, final int bStart, final int bEnd, final String bText) {
      List<Suggestions> input = new ArrayList<>();
      input.add(new Suggestions(StringRange.between(aStart, aEnd), List.of(new Suggestion(StringRange.between(aStart, aEnd), aText))));
      input.add(new Suggestions(StringRange.between(bStart, bEnd), List.of(new Suggestion(StringRange.between(bStart, bEnd), bText))));
      Suggestions merged = Suggestions.merge(command, input);
      StringBuilder out = new StringBuilder();
      out.append("MERGE\t").append(label).append('\t').append(merged.getRange().getStart()).append('\t').append(merged.getRange().getEnd());
      for (Suggestion suggestion : merged.getList()) {
         out.append('\t').append(suggestion.getText());
      }
      System.out.println(out);
   }

   /** `Suggestion.apply`. */
   private static void applyCase(final String label, final String field, final int start, final int end, final String text) {
      Suggestion suggestion = new Suggestion(StringRange.between(start, end), text);
      System.out.println("APPLY\t" + label + '\t' + suggestion.apply(field));
   }

   /** `String.compareToIgnoreCase`, normalised to -1 / 0 / 1. */
   private static void compareCase(final String a, final String b) {
      int raw = a.compareToIgnoreCase(b);
      System.out.println("CMP\t" + a + '\t' + b + '\t' + Integer.signum(raw));
   }

   public static void main(final String[] args) {
      // The exact-match drop, and that it is case-sensitive.
      builderCase("exact-drop", "Steve", 0, "Steve", "Steven");
      builderCase("case-sensitive-drop", "steve", 0, "Steve");
      // A non-zero start measures `remaining`, i.e. the tail only.
      builderCase("tail-drop", "hi Steve", 3, "Steve");
      // The sort.
      builderCase("sort", "", 0, "zeta", "Alpha", "beta");
      builderCase("dedupe", "", 0, "a", "a", "b");
      // An underscore against a letter, which is where the double fold shows.
      builderCase("underscore-sort", "", 0, "AZb", "A_b");
      // An empty build.
      builderCase("empty", "hello wo", 6);

      mergeCase("expand", "say hello", 0, 9, "tell hello", 4, 9, "helium");

      applyCase("splice", "hello wo there", 6, 8, "world");
      applyCase("whole", "wo", 0, 2, "world");

      for (String[] pair : Arrays.asList(
         new String[] {"abc", "ABD"},
         new String[] {"ABC", "abc"},
         new String[] {"abc", "abcd"},
         new String[] {"", "a"},
         new String[] {"A_b", "AZb"},
         new String[] {"_", "a"},
         new String[] {"_", "A"},
         new String[] {"Alpha", "beta"}
      )) {
         compareCase(pair[0], pair[1]);
      }
   }
}
