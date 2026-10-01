/// Battery cutoff choices, expressed as a percentage of full capacity.
public enum BatterySleepThreshold: Int, CaseIterable, Sendable {
  case percent00 = 0
  case percent10 = 10
  case percent20 = 20
  case percent30 = 30
  case percent40 = 40
  case percent50 = 50
  case percent60 = 60
  case percent70 = 70
  case percent80 = 80
  case percent90 = 90
}
