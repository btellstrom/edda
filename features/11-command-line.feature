@pending
Feature: The command line and its screens

  Background:
    Given a 2024 campaign with a DM device and the characters "Grog" and "Grand Poobah"

  @justifies:SPEC903 @justifies:SPEC905
  Scenario: CL1 An exact name beats a prefix, an ambiguous one is refused
    When the DM runs "edda gr"
    Then the exit code is not 0
    And the output names "Grog" and "Grand Poobah"
    When the DM runs "edda grog"
    Then Grog's sheet is shown

  @justifies:SPEC908
  Scenario: CL2 Completion escapes spaces
    When bash completes "edda grand"
    Then it offers "Grand\ Poobah"

  @justifies:SPEC917
  Scenario: CL3 A closed pipe ends edda quietly
    When the DM runs "edda grog | head -1"
    Then edda prints nothing to its error output

  @justifies:SPEC920 @justifies:SPEC925
  Scenario: CL4 The line says who does what to whom as it is typed
    Given a fight with Vex as 1, Grog as 2 and Goblin as 3, on Vex's turn
    When the DM types "damage 3"
    Then the line below reads "Vex deals ? damage to Goblin — how much?"
    And Vex and Goblin light up in the initiative order

  @justifies:SPEC926
  Scenario: CL5 A name where a combatant goes completes to their number
    Given a fight with Vex as 1, Grog as 2 and Goblin as 3, on Vex's turn
    When the DM types "damage gob" and presses Tab
    Then the line reads "damage 3 "

  @justifies:SPEC932
  Scenario: CL6 Casting opens the caster's spellcasting
    Given a fight on Keyleth's turn
    When the DM types "cast moon"
    Then the side panel shows Keyleth's spellcasting with her slots
    And describes "Moonbeam"

  @justifies:SPEC418 @justifies:SPEC937
  Scenario: CL7 A cramped terminal still has the command line
    Given a terminal 40 columns by 12 rows
    When the DM opens the fight screen, and then the planning screen
    Then each shows its command line
