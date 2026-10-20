@pending
Feature: Take back what one did

  Background:
    Given a 2024 campaign with a DM device and a player device playing "Vex"
    And a started combat with Vex and Goblin 1 at 7 hit points

  @justifies:SPEC301 @justifies:SPEC303
  Scenario: UN1 Undo takes back one's own latest command only
    Given the player dealt 3 damage to Goblin 1
    And the DM then dealt 2 damage to Vex
    When the player enters "undo"
    Then Goblin 1 has 7 hit points
    And Vex still has the DM's 2 damage

  @justifies:SPEC304
  Scenario: UN2 Undoing damage keeps damage dealt since
    Given the DM dealt 4 damage to Goblin 1
    And the player then dealt 2 damage to Goblin 1
    When the DM enters "undo"
    Then Goblin 1 has 5 hit points

  @justifies:SPEC308
  Scenario: UN3 Undoing a turn after the fight moved on is refused
    Given the DM entered "next"
    And the player then entered "next"
    When the DM enters "undo"
    Then the undo is refused with "the combat has moved on since"

  @justifies:SPEC310
  Scenario: UN4 Undoing a removal brings the combatant back
    Given Goblin 1 has the effect "Prone"
    And the DM entered "remove 2"
    When the DM enters "undo"
    Then Goblin 1 is back in the fight with "Prone" and their place in the order

  @justifies:SPEC254 @justifies:SPEC312
  Scenario: UN5 Undoing a cast ends its effect and gives the slot back
    Given Vex cast "Hunter's Mark" on Goblin 1 with a 1st-level slot
    When the player enters "undo"
    Then "Hunter's Mark" is not on Goblin 1
    And Vex's sheet has the 1st-level slot back
