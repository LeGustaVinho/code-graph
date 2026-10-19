import { Inject, Injectable, NotFoundException } from "@nestjs/common";
import { PrismaClient } from "@prisma/client";
import { PaymentGateway } from "../payments/payment";

@Injectable()
export class InvoicesService {
  private prisma = new PrismaClient();

  constructor(@Inject("PAYMENT_GATEWAY") private readonly gateway: PaymentGateway) {}

  async findOne(id: string) {
    const invoice = await this.prisma.invoice.findUnique({ where: { id } });
    if (!invoice) {
      throw new NotFoundException(`Invoice ${id} not found`);
    }
    return invoice;
  }

  async pay(id: string) {
    const invoice = await this.findOne(id);
    await this.gateway.charge(invoice.amount);
    return this.prisma.invoice.update({ where: { id }, data: { paid: true } });
  }
}
